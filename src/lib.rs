use serde::{Deserialize, Serialize};
use zed_extension_api::{
    self as zed, current_platform, lsp::CompletionKind, serde_json, CodeLabel, CodeLabelSpan,
    DebugAdapterBinary, LanguageServerId, Os, StartDebuggingRequestArguments,
    StartDebuggingRequestArgumentsRequest, Worktree,
};

#[derive(Debug, Deserialize, Serialize)]
struct PlaydateDebugConfig {
    request: String,
    #[serde(
        default = "default_game_path",
        skip_serializing_if = "Option::is_none",
        rename = "gamePath"
    )]
    game_path: Option<String>,
    #[serde(
        default = "default_source_path",
        skip_serializing_if = "Option::is_none",
        rename = "sourcePath"
    )]
    source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "sdkPath")]
    sdk_path: Option<String>,
}

fn default_game_path() -> Option<String> {
    Some("$ZED_WORKTREE_ROOT/builds/Game.pdx".to_string())
}

fn default_source_path() -> Option<String> {
    Some("$ZED_WORKTREE_ROOT/source".to_string())
}

#[derive(Default)]
struct PlaydateExtension {
    cached_binary_path: Option<String>,
    cached_luacats_path: Option<String>,
}

impl PlaydateExtension {
    const ADAPTER_NAME: &str = "Playdate";
    const LSP_SERVER_ID: &str = "playdate-lua-language-server";

    /// Detect the Playdate SDK path from various sources
    fn detect_sdk_path(&self, worktree: &Worktree) -> Result<String, String> {
        // 1. Check PLAYDATE_SDK_PATH environment variable from shell
        // Extension settings in extension.toml are also exposed as environment variables
        let env_vars = worktree.shell_env();
        for (key, value) in &env_vars {
            if key == "PLAYDATE_SDK_PATH" && !value.is_empty() {
                return Ok(value.clone());
            }
        }

        // 2. Check standard installation locations based on platform
        let (os, _arch) = current_platform();
        let home = env_vars
            .iter()
            .find(|(k, _)| k == "HOME" || k == "USERPROFILE")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");

        // 3. Return the standard installation path based on platform
        match os {
            Os::Mac => Ok(format!("{}/Developer/PlaydateSDK", home)),
            Os::Linux => Ok(format!("{}/.local/share/playdate-sdk", home)),
            Os::Windows => Ok(format!("{}\\Documents\\PlaydateSDK", home)),
        }
    }

    /// Get the path to the Playdate Simulator executable
    fn get_simulator_path(&self, worktree: &Worktree) -> Result<String, String> {
        let sdk_path = self.detect_sdk_path(worktree)?;
        let (os, _arch) = current_platform();

        match os {
            Os::Mac => Ok(format!(
                "{}/bin/Playdate Simulator.app/Contents/MacOS/Playdate Simulator",
                sdk_path
            )),
            Os::Linux => Ok(format!("{}/bin/PlaydateSimulator", sdk_path)),
            Os::Windows => Ok(format!("{}\\bin\\PlaydateSimulator.exe", sdk_path)),
        }
    }

    /// Get the request type for the Playdate Simulator
    fn get_request_type(
        &self,
        config: &PlaydateDebugConfig,
    ) -> Result<zed::StartDebuggingRequestArgumentsRequest, String> {
        // Check if the configuration specifies "attach" mode
        match config.request.as_str() {
            "attach" => Ok(StartDebuggingRequestArgumentsRequest::Attach),
            "launch" => Ok(StartDebuggingRequestArgumentsRequest::Launch),
            _ => Err(format!(
                "Invalid request type '{}'. Expected 'launch' or 'attach'",
                config.request
            )),
        }
    }

    fn playdate_luacats_path(&mut self, worktree: &Worktree) -> Result<String, String> {
        if let Some(path) = &self.cached_luacats_path {
            return Ok(path.clone());
        }

        // Find the pdc command and get version from it
        let pdc_path = worktree.which("pdc").ok_or_else(|| {
            let err = "pdc command not found in PATH".to_string();
            err
        })?;

        // Run pdc --version to get the SDK version
        let output = zed::Command::new(&pdc_path)
            .arg("--version")
            .output()
            .map_err(|e| {
                let err = format!("failed to run pdc --version: {}", e);
                err
            })?;

        let sdk_version = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // playdate-luacats uses tags like "v2.5.0-luacats1"
        // We'll try luacats1 first, and could fallback to other suffixes if needed
        let luacats_tag = format!("v{}-luacats1", sdk_version);

        let version_dir = format!("playdate-luacats-{}-luacats1", sdk_version);

        // Try to download luacats. The download will be skipped if it already exists.
        // Download the source tarball for the specific tag
        let tarball_url = format!(
            "https://github.com/notpeter/playdate-luacats/archive/refs/tags/{}.tar.gz",
            luacats_tag
        );

        zed::download_file(&tarball_url, ".", zed::DownloadedFileType::GzipTar).map_err(|e| {
            let err = format!(
                "failed to download playdate-luacats tag {}: {}",
                luacats_tag, e
            );
            err
        })?;

        // Convert relative path to absolute path using current_dir()
        // The extension work directory is where download_file extracts files
        let extension_dir = std::env::current_dir()
            .map_err(|e| format!("Failed to get extension directory: {}", e))?;

        let library_path = extension_dir
            .join(&version_dir)
            .join("library")
            .to_string_lossy()
            .into_owned();

        self.cached_luacats_path = Some(library_path.clone());
        Ok(library_path)
    }

    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String, String> {
        if let Some(path) = worktree.which("lua-language-server") {
            return Ok(path);
        }

        if let Some(path) = &self.cached_binary_path {
            return Ok(path.clone());
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = zed::latest_github_release(
            "LuaLS/lua-language-server",
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (platform, arch) = zed::current_platform();
        let asset_name = format!(
            "lua-language-server-{version}-{os}-{arch}.{extension}",
            version = release.version,
            os = match platform {
                zed::Os::Mac => "darwin",
                zed::Os::Linux => "linux",
                zed::Os::Windows => "win32",
            },
            arch = match arch {
                zed::Architecture::Aarch64 => "arm64",
                zed::Architecture::X8664 => "x64",
                zed::Architecture::X86 => return Err("unsupported platform x86".into()),
            },
            extension = match platform {
                zed::Os::Mac | zed::Os::Linux => "tar.gz",
                zed::Os::Windows => "zip",
            },
        );

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("no asset found matching {:?}", asset_name))?;

        let version_dir = format!("lua-language-server-{}", release.version);
        let binary_path = format!(
            "{version_dir}/bin/lua-language-server{extension}",
            extension = match platform {
                zed::Os::Mac | zed::Os::Linux => "",
                zed::Os::Windows => ".exe",
            },
        );

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::Downloading,
        );

        zed::download_file(
            &asset.download_url,
            &version_dir,
            match platform {
                zed::Os::Mac | zed::Os::Linux => zed::DownloadedFileType::GzipTar,
                zed::Os::Windows => zed::DownloadedFileType::Zip,
            },
        )
        .map_err(|e| format!("failed to download file: {e}"))?;

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl zed::Extension for PlaydateExtension {
    fn new() -> Self {
        Self::default()
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<zed_extension_api::Command, String> {
        if language_server_id.as_ref() != Self::LSP_SERVER_ID {
            return Err(format!(
                "Unsupported language server ID: {}",
                language_server_id.as_ref()
            ));
        }

        Ok(zed::Command {
            command: self.language_server_binary_path(language_server_id, worktree)?,
            args: Default::default(),
            env: Default::default(),
        })
    }

    fn label_for_completion(
        &self,
        _language_server_id: &LanguageServerId,
        completion: zed::lsp::Completion,
    ) -> Option<CodeLabel> {
        match completion.kind? {
            CompletionKind::Method | CompletionKind::Function => {
                let name_len = completion.label.find('(').unwrap_or(completion.label.len());
                Some(CodeLabel {
                    spans: vec![CodeLabelSpan::code_range(0..completion.label.len())],
                    filter_range: (0..name_len).into(),
                    code: completion.label,
                })
            }
            CompletionKind::Field => Some(CodeLabel {
                spans: vec![CodeLabelSpan::literal(
                    completion.label.clone(),
                    Some("property".into()),
                )],
                filter_range: (0..completion.label.len()).into(),
                code: Default::default(),
            }),
            _ => None,
        }
    }

    fn label_for_symbol(
        &self,
        _language_server_id: &LanguageServerId,
        symbol: zed::lsp::Symbol,
    ) -> Option<CodeLabel> {
        let prefix = "let a = ";
        let suffix = match symbol.kind {
            zed::lsp::SymbolKind::Method => "()",
            _ => "",
        };
        let code = format!("{prefix}{}{suffix}", symbol.name);
        Some(CodeLabel {
            spans: vec![CodeLabelSpan::code_range(
                prefix.len()..code.len() - suffix.len(),
            )],
            filter_range: (0..symbol.name.len()).into(),
            code,
        })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        _worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>, String> {
        if language_server_id.as_ref() != Self::LSP_SERVER_ID {
            return Ok(None);
        }

        // Configure lua-language-server for Playdate SDK
        Ok(Some(serde_json::json!({
            "Lua": {
                "runtime": {
                    "version": "Lua 5.4",
                    "special": {
                        "import": "require"
                    },
                    "builtin": { "io": "disable", "os": "disable", "package": "disable" },
                    "nonstandardSymbol": ["+=", "-=", "*=", "/=", "//=", "%=", "<<=", ">>=", "&=", "|=", "^="],
                },
                "diagnostics": {
                    "globals": ["playdate", "import"],
                    "severity": { "duplicate-set-field": "Hint", "unknown-symbol": "Warning" }
                },
                "workspace": {
                    "library": [
                        // Users can add their Playdate SDK path here
                    ],
                    "checkThirdParty": false
                },
                "completion": {
                    "callSnippet": "Replace"
                }
            }
        })))
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>, String> {
        if language_server_id.as_ref() != Self::LSP_SERVER_ID {
            return Ok(None);
        }

        // Detect Playdate SDK path (reuse your existing detect_sdk_path method)
        let sdk_path = self.detect_sdk_path(worktree).ok();

        // Build library paths array
        let mut library_paths = Vec::new();

        // Add Playdate SDK CoreLibs if available
        if let Some(ref path) = sdk_path {
            let corelibs_path = format!("{}/CoreLibs", path);
            library_paths.push(corelibs_path);
        }

        // Add playdate-luacats type definitions (downloaded to work directory)
        let luacats_path = self.playdate_luacats_path(worktree)?;
        library_paths.push(luacats_path);

        Ok(Some(serde_json::json!({
            "Lua": {
                "runtime": {
                    "version": "Lua 5.4",
                    "special": {
                        "import": "require"
                    },
                    "builtin": { "io": "disable", "os": "disable", "package": "disable" },
                    "nonstandardSymbol": ["+=", "-=", "*=", "/=", "//=", "%=", "<<=", ">>=", "&=", "|=", "^="],
                },
                "diagnostics": {
                    "globals": ["playdate", "import"],
                    "disable": ["duplicate-set-field"],
                    "severity": { "unknown-symbol": "Hint" }
                },
                "workspace": {
                    "library": library_paths,
                    "checkThirdParty": false
                },
                "completion": {
                    "callSnippet": "Replace"
                }
            }
        })))
    }

    fn get_dap_binary(
        &mut self,
        adapter_name: String,
        config: zed::DebugTaskDefinition,
        _user_provided_debug_adapter_path: Option<String>,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::DebugAdapterBinary, String> {
        if adapter_name != Self::ADAPTER_NAME {
            return Err(format!("Unsupported adapter name: {adapter_name}"));
        }

        // Parse the config JSON using our custom type
        let debug_config: PlaydateDebugConfig = {
            let mut cfg: PlaydateDebugConfig = serde_json::from_str(&config.config)
                .map_err(|e| format!("Failed to parse debug configuration: {}", e))?;
            if cfg.sdk_path.is_none() {
                cfg.sdk_path = Some(self.detect_sdk_path(worktree)?);
            }

            let root_path = worktree.root_path();

            if let Some(source_path) = cfg.source_path {
                cfg.source_path = Some(source_path.replace("$ZED_WORKTREE_ROOT", &root_path));
            }

            if let Some(game_path) = cfg.game_path {
                cfg.game_path = Some(game_path.replace("$ZED_WORKTREE_ROOT", &root_path));
            }

            cfg
        };

        // Get the request type (launch vs attach)
        let request = self.get_request_type(&debug_config)?;

        // Determine connection parameters (allow override from config)
        let (host, port, timeout) = if let Some(tcp_connection) = config.tcp_connection {
            (
                tcp_connection.host.unwrap_or(0x7f000001),
                tcp_connection.port.unwrap_or(55934),
                tcp_connection.timeout.unwrap_or(5000), // Default timeout of 5 seconds
            )
        } else {
            (0x7f000001, 55934, 5000)
        };

        // For launch mode, we need to start the Playdate Simulator
        let (command, arguments, cwd) =
            if let StartDebuggingRequestArgumentsRequest::Launch = request {
                let simulator_path = self.get_simulator_path(worktree)?;

                // Get the program path (PDX bundle) from the configuration
                let game_path = debug_config
                    .game_path
                    .clone()
                    .ok_or_else(|| "No game_path specified in launch configuration".to_string())?;

                let (os, _arch) = current_platform();
                match os {
                    Os::Mac => (Some(simulator_path), vec![game_path], None),
                    Os::Linux => (Some(simulator_path), vec![game_path], None),
                    Os::Windows => (Some(simulator_path), vec![game_path], None),
                }
            } else {
                // Attach mode: no command to execute
                (None, vec![], None)
            };

        let final_config = serde_json::to_string(&debug_config)
            .map_err(|e| format!("Failed to serialize final config: {}", e))?;

        // Create the debug adapter binary configuration
        // We use TCP connection to the simulator's debug server
        Ok(DebugAdapterBinary {
            command,
            arguments,
            envs: vec![],
            cwd,
            connection: Some(zed::TcpArguments {
                host,
                port,
                timeout: Some(timeout),
            }),
            request_args: StartDebuggingRequestArguments {
                configuration: final_config,
                request,
            },
        })
    }

    fn dap_request_kind(
        &mut self,
        adapter_name: String,
        config: serde_json::Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        if adapter_name != Self::ADAPTER_NAME {
            return Err(format!("Unsupported adapter name: {adapter_name}"));
        }

        // Parse the config into our custom type
        let debug_config: PlaydateDebugConfig = serde_json::from_value(config)
            .map_err(|e| format!("Failed to parse debug configuration: {}", e))?;

        self.get_request_type(&debug_config)
    }
}

zed::register_extension!(PlaydateExtension);
