use std::env;
use zed_extension_api::{self as zed, LanguageServerId, Result};

struct KnipExtension;

impl zed::Extension for KnipExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // Prefer a globally installed knip-lsp binary
        if let Some(path) = worktree.which("knip-lsp") {
            return Ok(zed::Command {
                command: path,
                args: vec!["--stdio".to_string()],
                env: Default::default(),
            });
        }

        // Use the bundled server.js shipped with this extension
        let server_path = env::current_dir()
            .unwrap()
            .join("server")
            .join("server.js")
            .to_string_lossy()
            .to_string();

        Ok(zed::Command {
            command: zed::node_binary_path()?,
            args: vec![server_path, "--stdio".to_string()],
            env: Default::default(),
        })
    }
}

zed::register_extension!(KnipExtension);
