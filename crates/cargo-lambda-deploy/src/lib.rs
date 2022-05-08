use cargo_lambda_build::find_binary_archive;
use cargo_lambda_interactive::progress::Progress;
use cargo_lambda_metadata::cargo::root_package;
use cargo_lambda_remote::{aws_sdk_lambda::model::Architecture, RemoteConfig};
use clap::{Args, ValueHint};
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::Serialize;
use serde_json::ser::to_string_pretty;
use std::{fs::read, path::PathBuf};
use strum_macros::{Display, EnumString};

mod extensions;
mod functions;

#[derive(Clone, Debug, Display, EnumString)]
#[strum(ascii_case_insensitive)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Serialize)]
#[serde(untagged)]
enum DeployResult {
    Extension(extensions::DeployOutput),
    Function(functions::DeployOutput),
}

impl std::fmt::Display for DeployResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployResult::Extension(o) => o.fmt(f),
            DeployResult::Function(o) => o.fmt(f),
        }
    }
}

#[derive(Args, Clone, Debug)]
#[clap(name = "deploy")]
pub struct Deploy {
    #[clap(flatten)]
    remote_config: RemoteConfig,

    #[clap(flatten)]
    function_config: functions::FunctionDeployConfig,

    /// Directory where the lambda binaries are located
    #[clap(short, long, value_hint = ValueHint::DirPath)]
    lambda_dir: Option<PathBuf>,

    /// Path to Cargo.toml
    #[clap(
        long,
        value_name = "PATH",
        parse(from_os_str),
        default_value = "Cargo.toml"
    )]
    pub manifest_path: PathBuf,

    /// Name of the binary to deploy if it doesn't match the name that you want to deploy it with
    #[clap(long)]
    pub binary_name: Option<String>,

    /// S3 bucket to upload the code to
    #[clap(long)]
    pub s3_bucket: Option<String>,

    /// Whether the code that you're building is a Lambda Extension
    #[clap(long)]
    extension: bool,

    /// Format to render the output (text, or json)
    #[clap(long, default_value_t = OutputFormat::Text)]
    output_format: OutputFormat,

    /// Name of the function or extension to deploy
    #[clap(value_name = "NAME")]
    name: Option<String>,
}

impl Deploy {
    pub async fn run(&self) -> Result<()> {
        if self.function_config.enable_function_url && self.function_config.disable_function_url {
            return Err(miette::miette!("invalid options: --enable-function-url and --disable-function-url cannot be set together"));
        }

        let name = match &self.name {
            Some(name) => name.clone(),
            None => root_package(&self.manifest_path)?.name,
        };
        let binary_name = self.binary_name.as_deref().unwrap_or(&name);

        let progress = Progress::start("loading binary data");

        let archive = match find_binary_archive(binary_name, &self.lambda_dir, self.extension) {
            Ok(arc) => arc,
            Err(err) => {
                progress.finish_and_clear();
                return Err(err);
            }
        };

        let sdk_config = self.remote_config.to_sdk_config().await;
        let architecture = Architecture::from(archive.architecture.as_str());

        let binary_data = read(&archive.path)
            .into_diagnostic()
            .wrap_err("failed to read binary archive")?;

        let result = if self.extension {
            extensions::deploy(
                &name,
                &sdk_config,
                binary_data,
                architecture,
                &self.s3_bucket,
                &progress,
            )
            .await
        } else {
            functions::deploy(
                &name,
                &self.function_config,
                &self.remote_config,
                &sdk_config,
                &self.s3_bucket,
                binary_data,
                architecture,
                &progress,
            )
            .await
        };

        progress.finish_and_clear();
        let output = result?;

        match &self.output_format {
            OutputFormat::Text => println!("{output}"),
            OutputFormat::Json => {
                let text = to_string_pretty(&output)
                    .into_diagnostic()
                    .wrap_err("failed to serialize output into json")?;
                println!("{text}")
            }
        }

        Ok(())
    }
}
