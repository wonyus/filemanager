use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderType {
    AwsS3,
    CloudflareR2,
    Minio,
    Wasabi,
    CustomS3,
}

impl ProviderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwsS3 => "awsS3",
            Self::CloudflareR2 => "cloudflareR2",
            Self::Minio => "minio",
            Self::Wasabi => "wasabi",
            Self::CustomS3 => "customS3",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "awsS3" => Self::AwsS3,
            "cloudflareR2" => Self::CloudflareR2,
            "minio" => Self::Minio,
            "wasabi" => Self::Wasabi,
            _ => Self::CustomS3,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialMode {
    Static,
    TemporarySession,
}

impl CredentialMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::TemporarySession => "temporarySession",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AddressingStyle {
    VirtualHosted,
    Path,
}

impl AddressingStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VirtualHosted => "virtualHosted",
            Self::Path => "path",
        }
    }
}
