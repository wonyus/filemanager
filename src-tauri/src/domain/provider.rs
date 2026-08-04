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

    pub fn parse_known(value: &str) -> Option<Self> {
        match value {
            "awsS3" => Some(Self::AwsS3),
            "cloudflareR2" => Some(Self::CloudflareR2),
            "minio" => Some(Self::Minio),
            "wasabi" => Some(Self::Wasabi),
            "customS3" => Some(Self::CustomS3),
            _ => None,
        }
    }

    pub fn default_region(self) -> &'static str {
        match self {
            // AWS region is deliberately required from the user; silently
            // selecting a production region can connect to the wrong account.
            Self::AwsS3 => "",
            Self::CloudflareR2 => "auto",
            Self::Minio | Self::CustomS3 => "us-east-1",
            Self::Wasabi => "us-east-1",
        }
    }

    pub fn default_addressing_style(self) -> AddressingStyle {
        match self {
            Self::Minio => AddressingStyle::Path,
            _ => AddressingStyle::VirtualHosted,
        }
    }

    pub fn endpoint_for(self, region: &str, account_id: Option<&str>) -> Option<String> {
        match self {
            Self::AwsS3 => None,
            Self::CloudflareR2 => account_id
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!("https://{value}.r2.cloudflarestorage.com")),
            Self::Wasabi => Some(match region {
                "us-east-1" => "https://s3.wasabisys.com".to_string(),
                "us-east-2" => "https://s3.us-east-2.wasabisys.com".to_string(),
                "eu-central-1" => "https://s3.eu-central-1.wasabisys.com".to_string(),
                "ap-northeast-1" => "https://s3.ap-northeast-1.wasabisys.com".to_string(),
                "ap-southeast-1" => "https://s3.ap-southeast-1.wasabisys.com".to_string(),
                _ => return None,
            }),
            Self::Minio | Self::CustomS3 => None,
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
