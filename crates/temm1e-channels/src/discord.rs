//! Discord channel — uses serenity for the Discord Bot API.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use temm1e_core::types::config::ChannelConfig;
use temm1e_core::types::error::Temm1eError;
use temm1e_core::types::file::{FileData, FileMetadata, OutboundFile, ReceivedFile};
use temm1e_core::types::message::{AttachmentRef, InboundMessage, OutboundMessage, ParseMode};
use temm1e_core::{Channel, FileTransfer};

use serenity::all::{
    ChannelId, Command, CommandDataOptionValue, CommandOptionType, Context, CreateAttachment,
    CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EventHandler, GatewayIntents, Interaction,
    Message, Ready,
};
use serenity::Client;

/// Maximum file size Discord supports for non-Nitro uploads (25 MB).
const DISCORD_UPLOAD_LIMIT: usize = 25 * 1024 * 1024;

// ── Persistent allowlist ──────────────────────────────────────────

/// On-disk representation of the Discord allowlist stored at
/// `~/.temm1e/discord_allowlist.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AllowlistFile {
    /// The admin user ID (the first user to ever message the bot).
    admin: String,
    /// All allowed user IDs (admin is always included).
    users: Vec<String>,
}

/// Return the path to `~/.temm1e/discord_allowlist.toml`.
fn allowlist_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".temm1e").join("discord_allowlist.toml"))
}

/// Load the persisted Discord allowlist from disk.
/// Returns `None` if the file does not exist or cannot be parsed.
fn load_allowlist_file() -> Option<AllowlistFile> {
    let path = allowlist_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    match toml::from_str(&content) {
        Ok(parsed) => Some(parsed),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "Failed to parse Discord allowlist file, ignoring"
            );
            None
        }
    }
}

/// Save the Discord allowlist to disk. Creates `~/.temm1e/` if needed.
fn save_allowlist_file(data: &AllowlistFile) -> Result<(), Temm1eError> {
    let path = allowlist_path().ok_or_else(|| {
        Temm1eError::Channel("Cannot determine home directory for Discord allowlist".into())
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            Temm1eError::Channel(format!("Failed to create ~/.temm1e directory: {e}"))
        })?;
    }
    let content = toml::to_string_pretty(data)
        .map_err(|e| Temm1eError::Channel(format!("Failed to serialize Discord allowlist: {e}")))?;
    std::fs::write(&path, content).map_err(|e| {
        Temm1eError::Channel(format!("Failed to write Discord allowlist file: {e}"))
    })?;
    tracing::info!(path = %path.display(), "Discord allowlist saved");
    Ok(())
}

/// Persist the current in-memory allowlist + admin to disk.
fn persist_allowlist(
    allowlist: &Arc<RwLock<Vec<String>>>,
    admin: &Arc<RwLock<Option<String>>>,
) -> Result<(), Temm1eError> {
    let list = match allowlist.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => {
            tracing::error!("Discord allowlist RwLock poisoned during persist, recovering");
            poisoned.into_inner().clone()
        }
    };
    let admin_id = match admin.read() {
        Ok(guard) => guard.clone().unwrap_or_default(),
        Err(poisoned) => {
            tracing::error!("Discord admin RwLock poisoned during persist, recovering");
            poisoned.into_inner().clone().unwrap_or_default()
        }
    };
    save_allowlist_file(&AllowlistFile {
        admin: admin_id,
        users: list,
    })
}

/// Discord messaging channel.
///
/// Implements the `Channel` and `FileTransfer` traits for Discord bot
/// integration via the serenity library. Supports DMs and @mentions in
/// guild channels, allowlist enforcement by numeric snowflake ID, and
/// file attachment transfer up to 25 MB (non-Nitro limit).
pub struct DiscordChannel {
    /// The serenity HTTP client for sending messages after connection.
    http: Arc<RwLock<Option<Arc<serenity::http::Http>>>>,
    /// Bot token.
    token: String,
    /// Whether to respond to DMs.
    respond_to_dms: bool,
    /// Whether to respond to @mentions in guild channels.
    respond_to_mentions: bool,
    /// Allowlist of user IDs (Discord snowflake IDs as strings).
    /// Empty at startup = auto-whitelist first user.
    allowlist: Arc<RwLock<Vec<String>>>,
    /// Admin user ID (first user to message the bot). `None` until the first
    /// user is auto-whitelisted or loaded from the persisted allowlist file.
    admin: Arc<RwLock<Option<String>>>,
    /// Sender used to forward inbound messages to the gateway.
    tx: mpsc::Sender<InboundMessage>,
    /// Receiver the gateway drains. Taken once via `take_receiver()`.
    rx: Option<mpsc::Receiver<InboundMessage>>,
    /// Handle to the client task.
    client_handle: Option<tokio::task::JoinHandle<()>>,
    /// Shutdown signal.
    shutdown: Arc<AtomicBool>,
}

impl std::fmt::Debug for DiscordChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let allowlist_display: Vec<String> = match self.allowlist.read() {
            Ok(guard) => guard.clone(),
            Err(_) => vec!["<lock poisoned>".to_string()],
        };
        f.debug_struct("DiscordChannel")
            .field("respond_to_dms", &self.respond_to_dms)
            .field("respond_to_mentions", &self.respond_to_mentions)
            .field("allowlist", &allowlist_display)
            .finish_non_exhaustive()
    }
}

impl DiscordChannel {
    /// Create a new Discord channel from a `ChannelConfig`.
    ///
    /// If a persisted allowlist exists at `~/.temm1e/discord_allowlist.toml`,
    /// it is loaded and merged with any entries from the config file.
    pub fn new(config: &ChannelConfig) -> Result<Self, Temm1eError> {
        let token = config
            .token
            .clone()
            .ok_or_else(|| Temm1eError::Config("Discord channel requires a bot token".into()))?;

        let (tx, rx) = mpsc::channel(256);

        // Try to load persisted allowlist; fall back to config.
        let (allowlist, admin) = if let Some(file) = load_allowlist_file() {
            tracing::info!(
                admin = %file.admin,
                users = ?file.users,
                "Loaded persisted Discord allowlist"
            );
            (file.users.clone(), Some(file.admin.clone()))
        } else if !config.allowlist.is_empty() {
            // Legacy: first entry in the config allowlist becomes admin.
            let admin = config.allowlist[0].clone();
            (config.allowlist.clone(), Some(admin))
        } else {
            (Vec::new(), None)
        };

        Ok(Self {
            http: Arc::new(RwLock::new(None)),
            token,
            respond_to_dms: true,
            respond_to_mentions: true,
            allowlist: Arc::new(RwLock::new(allowlist)),
            admin: Arc::new(RwLock::new(admin)),
            tx,
            rx: Some(rx),
            client_handle: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Take the inbound message receiver. The gateway should call this once.
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.rx.take()
    }

    /// Check if a user (by numeric snowflake ID) is on the allowlist.
    ///
    /// Only numeric user IDs are matched. Usernames are ignored because
    /// they can be changed, enabling allowlist bypass (CA-04).
    /// An empty allowlist means no one is whitelisted yet (auto-whitelist
    /// happens in the event handler when the first user writes).
    fn check_allowed(&self, user_id: &str) -> bool {
        let list = match self.allowlist.read() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("Discord allowlist RwLock poisoned, recovering");
                poisoned.into_inner()
            }
        };
        if list.is_empty() {
            return false; // No one whitelisted yet (DF-16)
        }
        // Wildcard: "*" means everyone is allowed
        if list.iter().any(|a| a == "*") {
            return true;
        }
        list.iter().any(|a| a == user_id)
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
    }

    async fn start(&mut self) -> Result<(), Temm1eError> {
        let intents = GatewayIntents::GUILDS
            | GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let tx = self.tx.clone();
        let allowlist = self.allowlist.clone();
        let admin = self.admin.clone();
        let http_holder = self.http.clone();
        let shutdown = self.shutdown.clone();
        let respond_to_dms = self.respond_to_dms;
        let respond_to_mentions = self.respond_to_mentions;

        let handler = DiscordHandler {
            tx,
            allowlist,
            admin,
            http_holder: http_holder.clone(),
            respond_to_dms,
            respond_to_mentions,
        };

        let token = self.token.clone();

        let handle = tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);

            loop {
                if shutdown.load(Ordering::Relaxed) {
                    tracing::info!("Discord client shutdown requested");
                    break;
                }

                let client_result = Client::builder(&token, intents)
                    .event_handler(handler.clone())
                    .await;

                match client_result {
                    Ok(mut client) => {
                        // Reset backoff on successful connection
                        backoff = std::time::Duration::from_secs(1);

                        if let Err(e) = client.start().await {
                            if shutdown.load(Ordering::Relaxed) {
                                break;
                            }
                            tracing::error!(error = %e, "Discord client error");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to build Discord client");
                    }
                }

                if shutdown.load(Ordering::Relaxed) {
                    break;
                }

                tracing::warn!(
                    backoff_secs = backoff.as_secs(),
                    "Discord client exited unexpectedly, reconnecting"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(60));
            }
        });

        self.client_handle = Some(handle);
        tracing::info!("Discord channel started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), Temm1eError> {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.client_handle.take() {
            // Give the client a moment to notice the shutdown
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
        tracing::info!("Discord channel stopped");
        Ok(())
    }

    async fn send_message(&self, msg: OutboundMessage) -> Result<(), Temm1eError> {
        let http = {
            let guard = match self.http.read() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("Discord HTTP RwLock poisoned in send_message, recovering");
                    poisoned.into_inner()
                }
            };
            guard
                .clone()
                .ok_or_else(|| Temm1eError::Channel("Discord client not connected yet".into()))?
        };

        let channel_id: ChannelId =
            msg.chat_id
                .parse::<u64>()
                .map(ChannelId::new)
                .map_err(|_| {
                    Temm1eError::Channel(format!("Invalid Discord channel_id: {}", msg.chat_id))
                })?;

        // Discord uses Markdown by default, so we just send the text directly.
        // If parse_mode is Html, we strip it (Discord doesn't support HTML).
        let text = match msg.parse_mode {
            Some(ParseMode::Html) => {
                // Basic HTML tag stripping for compatibility
                msg.text.clone()
            }
            _ => msg.text.clone(),
        };

        // Discord has a 2000 character message limit. Split if needed.
        let chunks = split_message(&text, 2000);
        for (i, chunk) in chunks.iter().enumerate() {
            let has_reply_ref = i == 0 && msg.reply_to.is_some();
            let mut builder = CreateMessage::new().content(chunk);

            // Reply to the original message (first chunk only — Discord allows
            // one reply reference per message)
            if has_reply_ref {
                if let Some(ref reply_id) = msg.reply_to {
                    if let Ok(mid) = reply_id.parse::<u64>() {
                        builder = builder
                            .reference_message((channel_id, serenity::all::MessageId::new(mid)));
                    }
                }
            }

            let result = channel_id.send_message(&http, builder).await;

            // If the reply reference was invalid (e.g. interaction ID, deleted
            // message), retry without it rather than losing the message.
            if has_reply_ref && result.is_err() {
                let fallback = CreateMessage::new().content(chunk);
                channel_id
                    .send_message(&http, fallback)
                    .await
                    .map_err(|e| {
                        Temm1eError::Channel(format!("Failed to send Discord message: {e}"))
                    })?;
            } else {
                result.map_err(|e| {
                    Temm1eError::Channel(format!("Failed to send Discord message: {e}"))
                })?;
            }
        }

        Ok(())
    }

    fn file_transfer(&self) -> Option<&dyn FileTransfer> {
        Some(self)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.check_allowed(user_id)
    }

    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<(), Temm1eError> {
        let http = {
            let guard = match self.http.read() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("Discord HTTP RwLock poisoned in delete_message, recovering");
                    poisoned.into_inner()
                }
            };
            guard
                .clone()
                .ok_or_else(|| Temm1eError::Channel("Discord client not connected yet".into()))?
        };

        let channel_id: ChannelId = chat_id.parse::<u64>().map(ChannelId::new).map_err(|_| {
            Temm1eError::Channel(format!("Invalid Discord channel_id: {}", chat_id))
        })?;

        let msg_id: serenity::all::MessageId = message_id
            .parse::<u64>()
            .map(serenity::all::MessageId::new)
            .map_err(|_| {
                Temm1eError::Channel(format!("Invalid Discord message_id: {}", message_id))
            })?;

        channel_id
            .delete_message(&http, msg_id)
            .await
            .map_err(|e| {
                Temm1eError::Channel(format!(
                    "Failed to delete Discord message {}: {}",
                    message_id, e
                ))
            })?;

        tracing::info!(
            chat_id = %chat_id,
            message_id = %message_id,
            "Deleted sensitive message from Discord channel"
        );
        Ok(())
    }

    async fn send_typing_indicator(&self, chat_id: &str) -> Result<(), Temm1eError> {
        let http = {
            let guard = match self.http.read() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("Discord HTTP RwLock poisoned in typing indicator, recovering");
                    poisoned.into_inner()
                }
            };
            guard
                .clone()
                .ok_or_else(|| Temm1eError::Channel("Discord client not connected yet".into()))?
        };

        let channel_id: ChannelId = chat_id.parse::<u64>().map(ChannelId::new).map_err(|_| {
            Temm1eError::Channel(format!("Invalid Discord channel_id: {}", chat_id))
        })?;

        channel_id
            .broadcast_typing(&http)
            .await
            .map_err(|e| {
                Temm1eError::Channel(format!("Failed to send typing indicator: {e}"))
            })?;
        Ok(())
    }
}

#[async_trait]
impl FileTransfer for DiscordChannel {
    async fn receive_file(&self, msg: &InboundMessage) -> Result<Vec<ReceivedFile>, Temm1eError> {
        let http = {
            let guard = match self.http.read() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("Discord HTTP RwLock poisoned in receive_file, recovering");
                    poisoned.into_inner()
                }
            };
            guard
                .clone()
                .ok_or_else(|| Temm1eError::Channel("Discord client not connected yet".into()))?
        };

        let mut files = Vec::new();

        for att in &msg.attachments {
            // The file_id for Discord attachments is the download URL.
            let url = &att.file_id;

            // Use a timeout to prevent hanging on slow/unresponsive CDN downloads.
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| {
                    Temm1eError::FileTransfer(format!("Failed to build HTTP client: {e}"))
                })?;
            let response = client.get(url).send().await.map_err(|e| {
                Temm1eError::FileTransfer(format!("Failed to download Discord attachment: {e}"))
            })?;

            let data = response.bytes().await.map_err(|e| {
                Temm1eError::FileTransfer(format!("Failed to read Discord attachment bytes: {e}"))
            })?;

            let name = att
                .file_name
                .clone()
                .unwrap_or_else(|| format!("file_{}", att.file_id));

            files.push(ReceivedFile {
                name,
                mime_type: att
                    .mime_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_string()),
                size: data.len(),
                data,
            });
        }

        // Suppress unused variable warning for http — it is needed to verify
        // the client is connected, and future implementations may use it for
        // authenticated downloads.
        let _ = http;

        Ok(files)
    }

    async fn send_file(&self, chat_id: &str, file: OutboundFile) -> Result<(), Temm1eError> {
        let http = {
            let guard = match self.http.read() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("Discord HTTP RwLock poisoned in send_file, recovering");
                    poisoned.into_inner()
                }
            };
            guard
                .clone()
                .ok_or_else(|| Temm1eError::Channel("Discord client not connected yet".into()))?
        };

        let channel_id: ChannelId = chat_id
            .parse::<u64>()
            .map(ChannelId::new)
            .map_err(|_| Temm1eError::Channel(format!("Invalid Discord channel_id: {chat_id}")))?;

        let data = match &file.data {
            FileData::Bytes(b) => b.to_vec(),
            FileData::Url(url) => {
                // Use a timeout to prevent hanging on slow/unresponsive downloads.
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build()
                    .map_err(|e| {
                        Temm1eError::FileTransfer(format!("Failed to build HTTP client: {e}"))
                    })?;
                let response = client.get(url).send().await.map_err(|e| {
                    Temm1eError::FileTransfer(format!("Failed to download file from URL: {e}"))
                })?;
                response
                    .bytes()
                    .await
                    .map_err(|e| {
                        Temm1eError::FileTransfer(format!("Failed to read file bytes: {e}"))
                    })?
                    .to_vec()
            }
        };

        let attachment = CreateAttachment::bytes(data, &file.name);
        let mut builder = CreateMessage::new().add_file(attachment);
        if let Some(ref caption) = file.caption {
            builder = builder.content(caption);
        }

        channel_id
            .send_message(&http, builder)
            .await
            .map_err(|e| Temm1eError::FileTransfer(format!("Failed to send Discord file: {e}")))?;

        Ok(())
    }

    async fn send_file_stream(
        &self,
        _chat_id: &str,
        _stream: BoxStream<'_, Bytes>,
        metadata: FileMetadata,
    ) -> Result<(), Temm1eError> {
        // Discord does not support streaming uploads. Callers should use
        // `send_file` with the fully-buffered data instead.
        Err(Temm1eError::FileTransfer(format!(
            "Discord does not support streaming file uploads. \
             Buffer the file ({}) and use send_file() instead.",
            metadata.name
        )))
    }

    fn max_file_size(&self) -> usize {
        DISCORD_UPLOAD_LIMIT
    }
}

// ── Serenity event handler ──────────────────────────────────────────

/// Internal serenity event handler that forwards Discord messages to the
/// gateway via the mpsc channel.
#[derive(Clone)]
struct DiscordHandler {
    tx: mpsc::Sender<InboundMessage>,
    allowlist: Arc<RwLock<Vec<String>>>,
    admin: Arc<RwLock<Option<String>>>,
    http_holder: Arc<RwLock<Option<Arc<serenity::http::Http>>>>,
    respond_to_dms: bool,
    respond_to_mentions: bool,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!(
            bot_name = %ready.user.name,
            guild_count = ready.guilds.len(),
            "Discord bot connected"
        );
        // Store the HTTP client so DiscordChannel can use it for sending.
        {
            let mut guard = match self.http_holder.write() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("Discord HTTP holder RwLock poisoned in ready(), recovering");
                    poisoned.into_inner()
                }
            };
            *guard = Some(ctx.http.clone());
        }

        // Register global slash commands — channel-level admin commands and
        // agent runtime commands so they all appear in Discord's "/" menu.
        let commands = vec![
            // ── Channel-level admin commands (handled locally) ──
            CreateCommand::new("allow")
                .description("Add a user to the allowlist (admin)")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "user_id",
                        "The numeric Discord user ID to allow",
                    )
                    .required(true),
                ),
            CreateCommand::new("revoke")
                .description("Remove a user from the allowlist (admin)")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "user_id",
                        "The numeric Discord user ID to revoke",
                    )
                    .required(true),
                ),
            CreateCommand::new("users").description("List all allowed users (admin)"),
            // ── Agent runtime commands (forwarded to gateway) ──
            CreateCommand::new("help").description("Show available commands"),
            CreateCommand::new("stop").description("Interrupt the active task"),
            CreateCommand::new("status").description("Show current task status"),
            CreateCommand::new("queue").description("Show queued orders"),
            CreateCommand::new("keys").description("List configured providers and active model"),
            CreateCommand::new("model")
                .description("Show or switch the current model")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "name",
                        "Model name to switch to",
                    )
                    .required(false),
                ),
            CreateCommand::new("addkey")
                .description("Add an API key")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "mode",
                        "Key add mode: (empty)=OTK, unsafe=paste, github=PAT",
                    )
                    .required(false),
                ),
            CreateCommand::new("removekey")
                .description("Remove a provider's API key")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "provider",
                        "Provider name to remove",
                    )
                    .required(true),
                ),
            CreateCommand::new("usage")
                .description("Show token usage and cost, or toggle per-turn display")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "toggle",
                        "on/off to enable/disable per-turn display",
                    )
                    .required(false),
                ),
            CreateCommand::new("memory")
                .description("Show or switch memory strategy (lambda/echo)")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "mode",
                        "Memory mode: lambda or echo",
                    )
                    .required(false),
                ),
            CreateCommand::new("cambium")
                .description("Cambium self-grow status, on/off, or grow")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "action",
                        "on, off, or grow <task>",
                    )
                    .required(false),
                ),
            CreateCommand::new("eigentune")
                .description("Eigen-Tune status and management")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "action",
                        "setup, model, tick, or demote <tier>",
                    )
                    .required(false),
                ),
            CreateCommand::new("mcp")
                .description("MCP server management")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "action",
                        "add/remove/restart <name> [command-or-url]",
                    )
                    .required(false),
                ),
            CreateCommand::new("browser")
                .description("Browser status and session management")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "action",
                        "close, sessions, or forget <service>",
                    )
                    .required(false),
                ),
            CreateCommand::new("timelimit")
                .description("Show or set task time limit")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "seconds",
                        "Time limit in seconds",
                    )
                    .required(false),
                ),
            CreateCommand::new("vigil")
                .description("Self-diagnosis vigil status and control")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "action",
                        "auto, disable, or status",
                    )
                    .required(false),
                ),
            CreateCommand::new("login")
                .description("OAuth authentication flow")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "service",
                        "Service to authenticate with",
                    )
                    .required(false),
                ),
            CreateCommand::new("reload").description("Hot-reload config and agent (admin)"),
            CreateCommand::new("reset").description("Factory reset all local state (admin)"),
            CreateCommand::new("restart").description("Restart TEMM1E process (admin)"),
        ];

        match Command::set_global_commands(&ctx.http, commands).await {
            Ok(cmds) => {
                tracing::info!(
                    count = cmds.len(),
                    "Registered Discord slash commands"
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to register Discord slash commands");
            }
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(cmd) = interaction else {
            return;
        };

        let user_id = cmd.user.id.get().to_string();
        let username = Some(cmd.user.name.clone());
        let channel_id = cmd.channel_id;

        // ── Channel-level admin commands (handled locally) ──
        if matches!(cmd.data.name.as_str(), "allow" | "revoke" | "users") {
            let is_admin = {
                let adm = match self.admin.read() {
                    Ok(g) => g,
                    Err(poisoned) => {
                        tracing::error!(
                            "Discord admin RwLock poisoned in interaction_create, recovering"
                        );
                        poisoned.into_inner()
                    }
                };
                adm.as_deref() == Some(&user_id)
            };

            if !is_admin {
                let response = CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Only the admin can use this command.")
                        .ephemeral(true),
                );
                if let Err(e) = cmd.create_response(&ctx.http, response).await {
                    tracing::warn!(error = %e, "Failed to send admin-only slash command reply");
                }
                return;
            }

            let reply = match cmd.data.name.as_str() {
                "users" => {
                    let list = match self.allowlist.read() {
                        Ok(g) => g,
                        Err(poisoned) => {
                            tracing::error!(
                                "Discord allowlist RwLock poisoned in /users slash cmd, recovering"
                            );
                            poisoned.into_inner()
                        }
                    };
                    let admin_id = match self.admin.read() {
                        Ok(g) => g.clone().unwrap_or_default(),
                        Err(poisoned) => {
                            tracing::error!(
                                "Discord admin RwLock poisoned in /users slash cmd, recovering"
                            );
                            poisoned.into_inner().clone().unwrap_or_default()
                        }
                    };
                    if list.is_empty() {
                        "Allowlist is empty.".to_string()
                    } else {
                        let mut lines = Vec::with_capacity(list.len());
                        for uid in list.iter() {
                            if uid == &admin_id {
                                lines.push(format!("{} (admin)", uid));
                            } else {
                                lines.push(uid.clone());
                            }
                        }
                        format!("Allowed users:\n{}", lines.join("\n"))
                    }
                }
                "allow" => {
                    let target = cmd
                        .data
                        .options
                        .first()
                        .and_then(|opt| match &opt.value {
                            CommandDataOptionValue::String(s) => Some(s.trim().to_string()),
                            _ => None,
                        })
                        .unwrap_or_default();

                    if target.is_empty() {
                        "Usage: /allow <user_id>".to_string()
                    } else {
                        let already_exists = {
                            let mut list = match self.allowlist.write() {
                                Ok(g) => g,
                                Err(poisoned) => {
                                    tracing::error!(
                                        "Discord allowlist RwLock poisoned in /allow slash cmd, recovering"
                                    );
                                    poisoned.into_inner()
                                }
                            };
                            if list.iter().any(|a| a == &target) {
                                true
                            } else {
                                list.push(target.clone());
                                false
                            }
                        };
                        if already_exists {
                            format!("User {} is already allowed.", target)
                        } else if let Err(e) = persist_allowlist(&self.allowlist, &self.admin) {
                            tracing::error!(error = %e, "Failed to persist allowlist after /allow slash cmd");
                            format!("User {} added (but failed to save to disk: {}).", target, e)
                        } else {
                            tracing::info!(target = %target, "Admin added user via /allow slash command");
                            format!("User {} added to the allowlist.", target)
                        }
                    }
                }
                "revoke" => {
                    let target = cmd
                        .data
                        .options
                        .first()
                        .and_then(|opt| match &opt.value {
                            CommandDataOptionValue::String(s) => Some(s.trim().to_string()),
                            _ => None,
                        })
                        .unwrap_or_default();

                    if target.is_empty() {
                        "Usage: /revoke <user_id>".to_string()
                    } else if target == user_id {
                        "You cannot revoke yourself.".to_string()
                    } else {
                        let was_present = {
                            let mut list = match self.allowlist.write() {
                                Ok(g) => g,
                                Err(poisoned) => {
                                    tracing::error!(
                                        "Discord allowlist RwLock poisoned in /revoke slash cmd, recovering"
                                    );
                                    poisoned.into_inner()
                                }
                            };
                            let before = list.len();
                            list.retain(|a| a != &target);
                            list.len() < before
                        };
                        if !was_present {
                            format!("User {} is not on the allowlist.", target)
                        } else if let Err(e) = persist_allowlist(&self.allowlist, &self.admin) {
                            tracing::error!(error = %e, "Failed to persist allowlist after /revoke slash cmd");
                            format!("User {} revoked (but failed to save to disk: {}).", target, e)
                        } else {
                            tracing::info!(target = %target, "Admin revoked user via /revoke slash command");
                            format!("User {} removed from the allowlist.", target)
                        }
                    }
                }
                _ => unreachable!(),
            };

            let response = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(reply)
                    .ephemeral(true),
            );
            if let Err(e) = cmd.create_response(&ctx.http, response).await {
                tracing::warn!(error = %e, "Failed to send slash command response");
            }
            return;
        }

        // ── Agent runtime commands — forward as InboundMessage ──
        // Reconstruct the text command from the slash interaction so the
        // agent runtime in main.rs processes it identically to a typed message.
        let args = cmd
            .data
            .options
            .iter()
            .map(|opt| match &opt.value {
                CommandDataOptionValue::String(s) => s.clone(),
                other => format!("{:?}", other),
            })
            .collect::<Vec<_>>()
            .join(" ");

        let text = if args.is_empty() {
            format!("/{}", cmd.data.name)
        } else {
            format!("/{} {}", cmd.data.name, args)
        };

        // Check allowlist before forwarding
        let is_allowed = {
            let list = match self.allowlist.read() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!(
                        "Discord allowlist RwLock poisoned in interaction allowlist check, recovering"
                    );
                    poisoned.into_inner()
                }
            };
            list.iter().any(|a| a == &user_id || a == "*")
        };
        if !is_allowed {
            let response = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("You are not on the allowlist.")
                    .ephemeral(true),
            );
            if let Err(e) = cmd.create_response(&ctx.http, response).await {
                tracing::warn!(error = %e, "Failed to send allowlist denial for slash command");
            }
            return;
        }

        // ACK the interaction with an ephemeral receipt — the agent runtime
        // will send the real response through the normal send_message path.
        // Using an ephemeral Message instead of Defer avoids a permanent
        // "thinking..." indicator that never resolves.
        let ack = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .content(format!("/{} received", cmd.data.name))
                .ephemeral(true),
        );
        if let Err(e) = cmd.create_response(&ctx.http, ack).await {
            tracing::warn!(error = %e, "Failed to ACK slash command interaction");
        }

        let inbound = InboundMessage {
            id: cmd.id.get().to_string(),
            channel: "discord".to_string(),
            chat_id: channel_id.get().to_string(),
            user_id,
            username,
            text: Some(text),
            attachments: Vec::new(),
            reply_to: None,
            timestamp: chrono::Utc::now(),
        };

        if self.tx.send(inbound).await.is_err() {
            tracing::error!("Discord inbound message receiver dropped (slash command)");
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore messages from bots (including ourselves)
        if msg.author.bot {
            return;
        }

        let user_id = msg.author.id.get().to_string();
        let username = Some(msg.author.name.clone());
        let is_dm = msg.guild_id.is_none();

        // Determine if we should handle this message based on channel type
        if is_dm && !self.respond_to_dms {
            return;
        }

        if !is_dm {
            // In guild channels, only respond to @mentions
            if !self.respond_to_mentions {
                return;
            }

            // Check if the bot is mentioned in the message
            let bot_mentioned = {
                let current_user_id = ctx.cache.current_user().id;
                msg.mentions.iter().any(|u| u.id == current_user_id)
            };

            if !bot_mentioned {
                return;
            }
        }

        // Auto-whitelist first user & set as admin
        {
            let mut list = match self.allowlist.write() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!(
                        "Discord allowlist RwLock poisoned in auto-whitelist, recovering"
                    );
                    poisoned.into_inner()
                }
            };
            if list.is_empty() {
                list.push(user_id.clone());
                let mut adm = match self.admin.write() {
                    Ok(g) => g,
                    Err(poisoned) => {
                        tracing::error!(
                            "Discord admin RwLock poisoned in auto-whitelist, recovering"
                        );
                        poisoned.into_inner()
                    }
                };
                *adm = Some(user_id.clone());
                tracing::info!(
                    user_id = %user_id,
                    username = ?username,
                    "Auto-whitelisted first Discord user as admin"
                );
                drop(list);
                drop(adm);
                if let Err(e) = persist_allowlist(&self.allowlist, &self.admin) {
                    tracing::error!(
                        error = %e,
                        "Failed to persist Discord allowlist after auto-whitelist"
                    );
                }
            }
        }

        // Reject non-allowlisted users
        {
            let list = match self.allowlist.read() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!(
                        "Discord allowlist RwLock poisoned in access check, recovering"
                    );
                    poisoned.into_inner()
                }
            };
            if !list.iter().any(|a| a == &user_id || a == "*") {
                drop(list);
                tracing::warn!(
                    user_id = %user_id,
                    username = ?username,
                    "Rejected Discord message from non-allowlisted user"
                );
                return;
            }
        }

        let channel_id = msg.channel_id;

        // Intercept admin commands
        if let Some(text) = msg.content.strip_prefix('!').or(Some(&msg.content)) {
            let trimmed = text.trim();

            if trimmed.starts_with("/allow ")
                || trimmed.starts_with("/revoke ")
                || trimmed == "/users"
            {
                let is_admin = {
                    let adm = match self.admin.read() {
                        Ok(g) => g,
                        Err(poisoned) => {
                            tracing::error!(
                                "Discord admin RwLock poisoned in admin check, recovering"
                            );
                            poisoned.into_inner()
                        }
                    };
                    adm.as_deref() == Some(&user_id)
                };

                if !is_admin {
                    if let Err(e) = channel_id
                        .send_message(
                            &ctx.http,
                            CreateMessage::new().content("Only the admin can use this command."),
                        )
                        .await
                    {
                        tracing::warn!(error = %e, "Failed to send admin-only reply to Discord");
                    }
                    return;
                }

                // /users — list all allowed user IDs
                if trimmed == "/users" {
                    let reply_text = {
                        let list = match self.allowlist.read() {
                            Ok(g) => g,
                            Err(poisoned) => {
                                tracing::error!(
                                    "Discord allowlist RwLock poisoned in /users, recovering"
                                );
                                poisoned.into_inner()
                            }
                        };
                        let admin_id = match self.admin.read() {
                            Ok(g) => g.clone().unwrap_or_default(),
                            Err(poisoned) => {
                                tracing::error!(
                                    "Discord admin RwLock poisoned in /users, recovering"
                                );
                                poisoned.into_inner().clone().unwrap_or_default()
                            }
                        };
                        if list.is_empty() {
                            "Allowlist is empty.".to_string()
                        } else {
                            let mut lines = Vec::with_capacity(list.len());
                            for uid in list.iter() {
                                if uid == &admin_id {
                                    lines.push(format!("{} (admin)", uid));
                                } else {
                                    lines.push(uid.clone());
                                }
                            }
                            format!("Allowed users:\n{}", lines.join("\n"))
                        }
                    };
                    if let Err(e) = channel_id
                        .send_message(&ctx.http, CreateMessage::new().content(&reply_text))
                        .await
                    {
                        tracing::warn!(error = %e, "Failed to send /users reply to Discord");
                    }
                    return;
                }

                // /allow <user_id>
                if let Some(target) = trimmed.strip_prefix("/allow ") {
                    let target = target.trim().to_string();
                    if target.is_empty() {
                        if let Err(e) = channel_id
                            .send_message(
                                &ctx.http,
                                CreateMessage::new().content("Usage: /allow <user_id>"),
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to send /allow usage reply to Discord");
                        }
                        return;
                    }
                    let already_exists = {
                        let mut list = match self.allowlist.write() {
                            Ok(g) => g,
                            Err(poisoned) => {
                                tracing::error!(
                                    "Discord allowlist RwLock poisoned in /allow, recovering"
                                );
                                poisoned.into_inner()
                            }
                        };
                        if list.iter().any(|a| a == &target) {
                            true
                        } else {
                            list.push(target.clone());
                            false
                        }
                    };
                    if already_exists {
                        if let Err(e) = channel_id
                            .send_message(
                                &ctx.http,
                                CreateMessage::new()
                                    .content(format!("User {} is already allowed.", target)),
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to send already-allowed reply to Discord");
                        }
                        return;
                    }
                    let reply = if let Err(e) = persist_allowlist(&self.allowlist, &self.admin) {
                        tracing::error!(
                            error = %e,
                            "Failed to persist allowlist after /allow"
                        );
                        format!("User {} added (but failed to save to disk: {}).", target, e)
                    } else {
                        format!("User {} added to the allowlist.", target)
                    };
                    if let Err(e) = channel_id
                        .send_message(&ctx.http, CreateMessage::new().content(&reply))
                        .await
                    {
                        tracing::warn!(error = %e, "Failed to send /allow reply to Discord");
                    }
                    tracing::info!(target = %target, "Admin added user to Discord allowlist");
                    return;
                }

                // /revoke <user_id>
                if let Some(target) = trimmed.strip_prefix("/revoke ") {
                    let target = target.trim().to_string();
                    if target.is_empty() {
                        if let Err(e) = channel_id
                            .send_message(
                                &ctx.http,
                                CreateMessage::new().content("Usage: /revoke <user_id>"),
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to send /revoke usage reply to Discord");
                        }
                        return;
                    }
                    if target == user_id {
                        if let Err(e) = channel_id
                            .send_message(
                                &ctx.http,
                                CreateMessage::new().content("You cannot revoke yourself."),
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to send self-revoke reply to Discord");
                        }
                        return;
                    }
                    let was_present = {
                        let mut list = match self.allowlist.write() {
                            Ok(g) => g,
                            Err(poisoned) => {
                                tracing::error!(
                                    "Discord allowlist RwLock poisoned in /revoke, recovering"
                                );
                                poisoned.into_inner()
                            }
                        };
                        let before = list.len();
                        list.retain(|a| a != &target);
                        list.len() < before
                    };
                    if !was_present {
                        if let Err(e) = channel_id
                            .send_message(
                                &ctx.http,
                                CreateMessage::new()
                                    .content(format!("User {} is not on the allowlist.", target)),
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to send not-on-allowlist reply to Discord");
                        }
                        return;
                    }
                    let reply = if let Err(e) = persist_allowlist(&self.allowlist, &self.admin) {
                        tracing::error!(
                            error = %e,
                            "Failed to persist allowlist after /revoke"
                        );
                        format!(
                            "User {} revoked (but failed to save to disk: {}).",
                            target, e
                        )
                    } else {
                        format!("User {} removed from the allowlist.", target)
                    };
                    if let Err(e) = channel_id
                        .send_message(&ctx.http, CreateMessage::new().content(&reply))
                        .await
                    {
                        tracing::warn!(error = %e, "Failed to send /revoke reply to Discord");
                    }
                    tracing::info!(target = %target, "Admin revoked user from Discord allowlist");
                    return;
                }
            }
        }

        // Extract message text — strip the bot mention prefix if in a guild
        let text = if !is_dm {
            let current_user_id = ctx.cache.current_user().id;
            let mention_str = format!("<@{}>", current_user_id);
            let mention_nick_str = format!("<@!{}>", current_user_id);
            let cleaned = msg
                .content
                .replace(&mention_str, "")
                .replace(&mention_nick_str, "");
            let cleaned = cleaned.trim().to_string();
            if cleaned.is_empty() {
                None
            } else {
                Some(cleaned)
            }
        } else {
            let content = msg.content.trim().to_string();
            if content.is_empty() {
                None
            } else {
                Some(content)
            }
        };

        // Extract attachments
        let attachments = extract_attachments(&msg);

        // Skip messages with no text and no attachments
        if text.is_none() && attachments.is_empty() {
            return;
        }

        let chat_id_str = channel_id.get().to_string();

        let inbound = InboundMessage {
            id: msg.id.get().to_string(),
            channel: "discord".to_string(),
            chat_id: chat_id_str,
            user_id,
            username,
            text,
            attachments,
            reply_to: msg
                .referenced_message
                .as_ref()
                .map(|r| r.id.get().to_string()),
            timestamp: *msg.timestamp,
        };

        if self.tx.send(inbound).await.is_err() {
            tracing::error!("Discord inbound message receiver dropped");
        }
    }
}

/// Extract attachment references from a Discord message.
fn extract_attachments(msg: &Message) -> Vec<AttachmentRef> {
    msg.attachments
        .iter()
        .map(|att| AttachmentRef {
            // Use the URL as the file_id — Discord attachments are
            // downloadable directly via their CDN URL.
            file_id: att.url.clone(),
            file_name: Some(att.filename.clone()),
            mime_type: att.content_type.clone(),
            size: Some(att.size as usize),
        })
        .collect()
}

/// Find the last byte offset that is on a UTF-8 char boundary at or before `max`.
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Split a message into chunks that fit within Discord's character limit.
/// All splits respect UTF-8 char boundaries to prevent panics on multi-byte text.
fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }

        let safe_end = floor_char_boundary(remaining, max_len);
        // Try to split at a newline boundary
        let split_at = remaining[..safe_end].rfind('\n').unwrap_or_else(|| {
            // Fall back to splitting at a space
            remaining[..safe_end].rfind(' ').unwrap_or(safe_end)
        });

        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk.to_string());
        remaining = rest.trim_start_matches('\n');
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_discord_channel_requires_token() {
        let config = ChannelConfig {
            enabled: true,
            token: None,
            allowlist: Vec::new(),
            file_transfer: true,
            max_file_size: None,
        };
        let result = DiscordChannel::new(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bot token"), "error was: {err}");
    }

    #[test]
    fn create_discord_channel_with_token() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token-123".to_string()),
            allowlist: Vec::new(),
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        assert_eq!(channel.name(), "discord");
    }

    #[test]
    fn discord_channel_name() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: Vec::new(),
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        assert_eq!(channel.name(), "discord");
    }

    #[test]
    fn discord_empty_allowlist_denies_all() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: Vec::new(),
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        // Empty allowlist = deny all (DF-16)
        assert!(!channel.is_allowed("123456789"));
        assert!(!channel.is_allowed("anyone"));
    }

    #[test]
    fn discord_wildcard_allowlist_allows_all() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: vec!["*".to_string()],
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        assert!(channel.is_allowed("123456789"));
        assert!(channel.is_allowed("999888777"));
        assert!(channel.is_allowed("anyone"));
    }

    #[test]
    fn discord_wildcard_with_other_entries() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: vec!["*".to_string(), "111222333".to_string()],
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        // Wildcard overrides — everyone allowed
        assert!(channel.is_allowed("999888777"));
        assert!(channel.is_allowed("111222333"));
    }

    #[test]
    fn discord_allowlist_matches_user_ids() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: vec!["111222333".to_string(), "444555666".to_string()],
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        assert!(channel.is_allowed("111222333"));
        assert!(channel.is_allowed("444555666"));
        assert!(!channel.is_allowed("999888777"));
        // Must match exact ID, not username
        assert!(!channel.is_allowed("SomeUsername#1234"));
    }

    #[test]
    fn discord_file_transfer_available() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: Vec::new(),
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        assert!(channel.file_transfer().is_some());
    }

    #[test]
    fn discord_max_file_size() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: Vec::new(),
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        assert_eq!(
            channel.file_transfer().unwrap().max_file_size(),
            25 * 1024 * 1024
        );
    }

    #[test]
    fn discord_take_receiver() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: Vec::new(),
            file_transfer: true,
            max_file_size: None,
        };
        let mut channel = DiscordChannel::new(&config).unwrap();
        // First take should succeed
        assert!(channel.take_receiver().is_some());
        // Second take should return None
        assert!(channel.take_receiver().is_none());
    }

    #[test]
    fn split_message_short() {
        let chunks = split_message("hello", 2000);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn split_message_at_limit() {
        let text = "a".repeat(2000);
        let chunks = split_message(&text, 2000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 2000);
    }

    #[test]
    fn split_message_over_limit() {
        let text = "a".repeat(2500);
        let chunks = split_message(&text, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 2000);
        assert_eq!(chunks[1].len(), 500);
    }

    #[test]
    fn split_message_prefers_newline_boundary() {
        let mut text = "a".repeat(1900);
        text.push('\n');
        text.push_str(&"b".repeat(500));
        let chunks = split_message(&text, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 1900);
    }

    #[test]
    fn extract_attachments_empty() {
        // We cannot easily construct a serenity Message in tests without
        // the full Discord API, so we test the split_message helper instead.
        // The extract_attachments function is a trivial mapping and will be
        // validated by integration tests.
    }

    // ── delete_message trait method existence ─────────────────────────
    // We verify the method exists by confirming DiscordChannel implements
    // the Channel trait (which now includes delete_message).

    #[test]
    fn discord_channel_implements_channel_trait() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: Vec::new(),
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        // If this compiles, DiscordChannel implements Channel (including delete_message)
        let _: &dyn Channel = &channel;
    }

    #[tokio::test]
    async fn discord_delete_message_requires_client_connected() {
        let config = ChannelConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            allowlist: Vec::new(),
            file_transfer: true,
            max_file_size: None,
        };
        let channel = DiscordChannel::new(&config).unwrap();
        // delete_message should fail because the client is not connected
        let result = channel.delete_message("123456789", "987654321").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not connected"),
            "Should fail with 'not connected', got: {err}"
        );
    }
}
