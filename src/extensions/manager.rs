use super::executor::ExtensionExecutor;
use anyhow::{Context, Result};
use dotenv_parser::parse_dotenv;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;
use tempfile;
use tokio::fs as tokio_fs;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use toml_edit::{DocumentMut, Item, Table};

const EXTENSIONS_REPO: &str = "https://github.com/ronnakamoto/bunsan-extensions";
const EXTENSIONS_DIR: &str = "extensions";
const DIST_DIR: &str = "dist";
const ENV_EXAMPLE: &str = ".env.example";
const PACKAGE_JSON: &str = "package.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterSource {
    Body,
    Header,
    Query,
    Path,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArrayStyle {
    Repeated,     // --param value1 --param value2
    Variadic,     // --param value1 value2
    Concatenated, // --param value1,value2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: String,
    pub required: bool,
    pub description: Option<String>,
    pub source: ParameterSource,
    #[serde(default)]
    pub array_style: Option<ArrayStyle>,
}

impl ParameterConfig {
    pub fn is_array_type(&self) -> bool {
        self.param_type.starts_with("array") || self.param_type.ends_with("[]")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub path: String,
    pub method: String,
    pub command: String,
    pub description: Option<String>,
    pub parameters: Option<Vec<ParameterConfig>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackageJson {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    main: String,
    bunsan: Option<BunsanConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BunsanConfig {
    binary_name: Option<String>,
    routes: Option<Vec<RouteConfig>>,
}

#[derive(Debug, Clone)]
pub struct ExtensionManager {
    pub config_path: PathBuf,
    routes: HashMap<String, Vec<RouteConfig>>,
    executor: ExtensionExecutor,
    extensions_path: PathBuf,
}

impl ExtensionManager {
    pub fn new(base_path: &Path, config_path: &Path) -> Self {
        let extensions_path = base_path.join(EXTENSIONS_DIR);
        fs::create_dir_all(&extensions_path).expect("Failed to create extensions directory");

        Self {
            executor: ExtensionExecutor::new(extensions_path.clone()),
            config_path: config_path.to_path_buf(),
            routes: HashMap::new(),
            extensions_path,
        }
    }

    pub async fn load_extensions(&mut self) -> Result<()> {
        info!(
            "Starting to load extensions from: {:?}",
            self.extensions_path
        );
        let mut entries = tokio_fs::read_dir(&self.extensions_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let package_json_path = entry.path();
                let package_json_file = package_json_path.join(PACKAGE_JSON);
                info!("Checking for package.json at: {:?}", package_json_file);
                if package_json_file.exists() {
                    info!("Found package.json, attempting to read");
                    match self.read_package_json(&package_json_path).await {
                        Ok(package_json) => {
                            info!("Successfully read package.json for: {}", package_json.name);
                            if let Some(bunsan_config) = &package_json.bunsan {
                                if let Some(routes) = &bunsan_config.routes {
                                    info!(
                                        "Loaded {} routes for extension: {}",
                                        routes.len(),
                                        package_json.name
                                    );
                                    for route in routes {
                                        info!("Route: {} {}", route.method, route.path);
                                    }
                                    self.routes
                                        .insert(package_json.name.clone(), routes.clone());
                                } else {
                                    warn!("No routes defined for extension: {}", package_json.name);
                                }
                            } else {
                                warn!(
                                    "No Bunsan configuration found for extension: {}",
                                    package_json.name
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                "Failed to load extension from {:?}: {}",
                                package_json_file, e
                            );
                        }
                    }
                } else {
                    warn!("No package.json found at: {:?}", package_json_file);
                }
            }
        }
        info!(
            "Finished loading extensions. Loaded {} extensions",
            self.routes.len()
        );
        Ok(())
    }

    pub fn get_all_routes(&self) -> &HashMap<String, Vec<RouteConfig>> {
        &self.routes
    }

    pub async fn install_extension(&mut self, extension_name: &str) -> Result<()> {
        info!("Starting installation of extension: {}", extension_name);
        let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;
        let repo_path = temp_dir.path();
        debug!("Temporary directory created at: {:?}", repo_path);

        // Clone the extensions repository
        self.clone_repo(repo_path).await?;
        debug!("Extensions repository cloned to temporary directory");

        // List contents of the cloned repository
        self.list_directory_contents(repo_path).await?;

        // Check if the extension exists
        let extension_path = repo_path.join(extension_name);
        if !extension_path.exists() {
            warn!("Extension '{}' not found in the repository", extension_name);
            let available_extensions = self.list_available_extensions(repo_path).await?;
            error!("Available extensions: {:?}", available_extensions);
            anyhow::bail!(
                "Extension '{}' not found. Available extensions: {:?}",
                extension_name,
                available_extensions
            );
        }

        // Read package.json to get the binary name
        let package_json = self.read_package_json(&extension_path).await?;
        let binary_name = package_json
            .bunsan
            .as_ref()
            .and_then(|config| config.binary_name.clone())
            .unwrap_or_else(|| package_json.name.clone());

        info!("Binary name for extension: {}", binary_name);

        // Copy all files from the dist folder to the Bunsan extensions directory
        let src_dist_path = extension_path.join(DIST_DIR);
        let dest_path = self.extensions_path.join(extension_name);

        self.copy_dir_all(&src_dist_path, &dest_path).await?;
        info!("Copied all files from dist folder to: {:?}", dest_path);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let platforms = ["linux-x64", "macos-arm64", "macos-x64"];
            for platform in platforms.iter() {
                let binary_path = dest_path
                    .join(format!("{}-{}", binary_name, platform))
                    .join(&binary_name);
                if binary_path.exists() {
                    let mut perms = tokio_fs::metadata(&binary_path).await?.permissions();
                    perms.set_mode(0o755); // rwxr-xr-x
                    tokio_fs::set_permissions(&binary_path, perms).await?;
                    info!("Set executable permissions for: {:?}", binary_path);
                }
            }
        }

        #[cfg(windows)]
        {
            // On Windows, executable permissions are handled by the file extension
            info!("Skipping executable permissions on Windows");
        }

        // Copy package.json to the extension directory
        let dest_package_json = dest_path.join(PACKAGE_JSON);
        tokio_fs::copy(extension_path.join(PACKAGE_JSON), &dest_package_json).await?;
        info!("Copied package.json to: {:?}", dest_package_json);

        // Read .env.example and update config.toml
        self.update_config_from_env_example(extension_name, &extension_path)
            .await?;
        info!("Updated config.toml with extension environment variables");

        // After successful installation, load the extension's routes
        let package_json = self.read_package_json(&dest_path).await?;
        if let Some(routes) = package_json.bunsan.and_then(|c| c.routes) {
            self.routes.insert(package_json.name.clone(), routes);
        }

        println!("Extension '{}' installed successfully", extension_name);
        println!(
            "Description: {}",
            package_json.description.unwrap_or_default()
        );
        println!("Version: {}", package_json.version);
        println!("Installed at: {:?}", dest_path);

        info!("Extension '{}' installation completed", extension_name);
        // The temporary directory will be automatically deleted when it goes out of scope
        Ok(())
    }

    fn copy_dir_all<'a>(
        &'a self,
        src: impl AsRef<Path> + 'a,
        dst: impl AsRef<Path> + 'a,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(async move {
            tokio_fs::create_dir_all(&dst).await?;
            let mut entries = tokio_fs::read_dir(src).await?;
            while let Some(entry) = entries.next_entry().await? {
                let ty = entry.file_type().await?;
                if ty.is_dir() {
                    self.copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))
                        .await?;
                } else {
                    tokio_fs::copy(entry.path(), dst.as_ref().join(entry.file_name())).await?;
                }
            }
            Ok(())
        })
    }

    async fn clone_repo(&self, path: &Path) -> Result<()> {
        info!("Cloning repository from: {}", EXTENSIONS_REPO);

        const MAX_RETRIES: u32 = 3;
        const TIMEOUT_SECONDS: u64 = 60;

        for attempt in 1..=MAX_RETRIES {
            info!("Clone attempt {} of {}", attempt, MAX_RETRIES);

            let clone_result = timeout(
                Duration::from_secs(TIMEOUT_SECONDS),
                TokioCommand::new("git")
                    .args(&[
                        "clone",
                        "--depth",
                        "1",
                        EXTENSIONS_REPO,
                        path.to_str().unwrap(),
                    ])
                    .output(),
            )
            .await;

            match clone_result {
                Ok(Ok(output)) => {
                    if output.status.success() {
                        info!("Repository cloned successfully");
                        return Ok(());
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        error!("Git clone failed: {}", stderr);
                        if attempt == MAX_RETRIES {
                            anyhow::bail!(
                                "Failed to clone extensions repository after {} attempts: {}",
                                MAX_RETRIES,
                                stderr
                            );
                        }
                    }
                }
                Ok(Err(e)) => {
                    error!("Error executing git command: {}", e);
                    if attempt == MAX_RETRIES {
                        anyhow::bail!(
                            "Failed to execute git command after {} attempts: {}",
                            MAX_RETRIES,
                            e
                        );
                    }
                }
                Err(_) => {
                    error!(
                        "Git clone operation timed out after {} seconds",
                        TIMEOUT_SECONDS
                    );
                    if attempt == MAX_RETRIES {
                        anyhow::bail!(
                            "Git clone operation timed out after {} attempts",
                            MAX_RETRIES
                        );
                    }
                }
            }

            // Wait before retrying
            tokio::time::sleep(Duration::from_secs(5)).await;
        }

        unreachable!("This point should never be reached due to the for loop's logic");
    }
    async fn list_directory_contents(&self, path: &Path) -> Result<()> {
        info!("Listing contents of directory: {:?}", path);
        let mut entries = tokio_fs::read_dir(path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_name = entry.file_name();
            let file_type = entry.file_type().await?;
            info!("{}: {:?}", file_name.to_string_lossy(), file_type);
        }
        Ok(())
    }

    async fn list_available_extensions(&self, repo_path: &Path) -> Result<Vec<String>> {
        let mut extensions = Vec::new();
        let mut entries = tokio_fs::read_dir(repo_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                extensions.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(extensions)
    }

    async fn read_package_json(&self, extension_path: &Path) -> Result<PackageJson> {
        let package_json_path = extension_path.join(PACKAGE_JSON);
        info!(
            "Attempting to read package.json from: {:?}",
            package_json_path
        );

        let package_json_content = tokio_fs::read_to_string(&package_json_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to read package.json file at {:?}",
                    package_json_path
                )
            })?;

        info!("Successfully read package.json content");
        debug!("package.json content: {}", package_json_content);

        let package_json: PackageJson =
            serde_json::from_str(&package_json_content).with_context(|| {
                format!(
                    "Failed to parse package.json content from {:?}",
                    package_json_path
                )
            })?;

        info!("Successfully parsed package.json");
        Ok(package_json)
    }

    async fn update_config_from_env_example(
        &self,
        extension_name: &str,
        extension_path: &Path,
    ) -> Result<()> {
        let env_example_path = extension_path.join(ENV_EXAMPLE);
        let env_content = tokio_fs::read_to_string(env_example_path).await?;
        let env_vars = parse_dotenv(&env_content)
            .map_err(|e| anyhow::anyhow!("Failed to parse .env file: {}", e))?;

        let config_content = tokio_fs::read_to_string(&self.config_path).await?;
        let mut doc = config_content.parse::<DocumentMut>()?;

        let extensions = doc["extensions"].or_insert(Item::Table(Table::new()));
        let extension_table = extensions
            .as_table_mut()
            .expect("extensions should be a table")
            .entry(extension_name)
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .expect("extension entry should be a table");

        extension_table.insert("name", toml_edit::value(extension_name));
        for (key, value) in env_vars {
            extension_table.insert(&key, toml_edit::value(value));
        }

        tokio_fs::write(&self.config_path, doc.to_string()).await?;

        Ok(())
    }

    pub async fn list_installed_extensions(&self) -> Result<Vec<PackageJson>> {
        let mut installed_extensions = Vec::new();

        let mut entries = tokio_fs::read_dir(&self.extensions_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let package_json_path = entry.path().join(PACKAGE_JSON);
                if package_json_path.exists() {
                    let package_json = self.read_package_json(&entry.path()).await?;
                    installed_extensions.push(package_json);
                }
            }
        }

        Ok(installed_extensions)
    }

    pub async fn uninstall_extension(&mut self, extension_name: &str) -> Result<()> {
        let extension_path = self.extensions_path.join(extension_name);
        if !extension_path.exists() {
            anyhow::bail!("Extension '{}' is not installed", extension_name);
        }

        // Remove the extension's routes
        self.routes.remove(extension_name);

        tokio_fs::remove_dir_all(extension_path).await?;

        // Remove extension from config.toml
        let config_content = tokio_fs::read_to_string(&self.config_path).await?;
        let mut doc = config_content.parse::<DocumentMut>()?;

        if let Some(extensions) = doc["extensions"].as_table_mut() {
            extensions.remove(extension_name);
        }

        tokio_fs::write(&self.config_path, doc.to_string()).await?;

        println!("Extension '{}' uninstalled successfully", extension_name);
        Ok(())
    }

    pub async fn run_extension(
        &self,
        extension_name: &str,
        command: &str,
        args: Vec<String>,
        env_vars: HashMap<String, String>,
    ) -> Result<String> {
        debug!("Running extension: {} {}", &extension_name, command);
        debug!("Arguments: {:?}", args);
        debug!("Environment variables: {:?}", env_vars);
        self.executor
            .execute(extension_name, command, args, env_vars)
    }
}
