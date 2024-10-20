use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ExtensionExecutor {
    extensions_path: PathBuf,
}

impl ExtensionExecutor {
    pub fn new(extensions_path: PathBuf) -> Self {
        Self { extensions_path }
    }

    pub fn execute(
        &self,
        extension_name: &str,
        command: &str,
        args: Vec<String>,
        env_vars: HashMap<String, String>,
    ) -> Result<String> {
        let extension_path = self.extensions_path.join(extension_name);
        let package_json: Value = serde_json::from_str(&std::fs::read_to_string(
            extension_path.join("package.json"),
        )?)?;

        let binary_name = package_json["bunsan"]["binary_name"]
            .as_str()
            .unwrap_or(extension_name);

        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        let platform_folder = match (os, arch) {
            ("linux", "x86_64") => format!("{}-linux-x64", binary_name),
            ("macos", "aarch64") => format!("{}-macos-arm64", binary_name),
            ("macos", "x86_64") => format!("{}-macos-x64", binary_name),
            ("windows", "x86_64") => format!("{}-win-x64.exe", binary_name),
            _ => anyhow::bail!("Unsupported platform: {}-{}", os, arch),
        };

        let binary_path = extension_path.join(platform_folder).join(binary_name);

        let mut cmd = Command::new(&binary_path);
        cmd.arg(command).args(args).envs(env_vars);

        let output = cmd.output()?;

        if output.status.success() {
            Ok(String::from_utf8(output.stdout)?)
        } else {
            Err(anyhow::anyhow!(
                "Extension execution failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }
}
