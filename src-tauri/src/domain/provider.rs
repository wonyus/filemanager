use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
struct WasabiPresetTable {
    version: u16,
    regions: BTreeMap<String, String>,
}

fn wasabi_endpoint(region: &str) -> Option<String> {
    let table: WasabiPresetTable = serde_json::from_str(include_str!("wasabi-presets.json"))
        .expect("embedded Wasabi preset table must be valid JSON");
    if table.version != 1 {
        return None;
    }
    table.regions.get(region).cloned()
}

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
            Self::Wasabi => wasabi_endpoint(region),
            Self::Minio | Self::CustomS3 => None,
        }
    }

    /// Capability defaults from the provider matrix in SDD section 42.
    /// `None` means that the capability is policy-/deployment-dependent and
    /// must be observed during a connection test. A baseline is never a
    /// substitute for the persisted result of a runtime probe.
    pub const fn capability_baseline(self) -> ProviderCapabilityBaseline {
        match self {
            Self::AwsS3 => ProviderCapabilityBaseline {
                can_list_buckets: None,
                can_head_bucket: None,
                supports_multipart_upload: Some(true),
                supports_multipart_copy: Some(true),
                supports_presigned_get: Some(true),
            },
            Self::CloudflareR2 => ProviderCapabilityBaseline {
                can_list_buckets: None,
                can_head_bucket: None,
                supports_multipart_upload: Some(true),
                supports_multipart_copy: None,
                supports_presigned_get: Some(true),
            },
            Self::Minio => ProviderCapabilityBaseline {
                can_list_buckets: None,
                can_head_bucket: None,
                supports_multipart_upload: None,
                supports_multipart_copy: None,
                supports_presigned_get: None,
            },
            Self::Wasabi => ProviderCapabilityBaseline {
                can_list_buckets: None,
                can_head_bucket: None,
                supports_multipart_upload: Some(true),
                supports_multipart_copy: None,
                supports_presigned_get: Some(true),
            },
            Self::CustomS3 => ProviderCapabilityBaseline {
                can_list_buckets: None,
                can_head_bucket: None,
                supports_multipart_upload: None,
                supports_multipart_copy: None,
                supports_presigned_get: None,
            },
        }
    }
}

/// The capability baseline is intentionally represented with `Option<bool>`
/// so unknown/provider-policy values cannot accidentally be treated as
/// supported by callers.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ProviderCapabilityBaseline {
    pub can_list_buckets: Option<bool>,
    pub can_head_bucket: Option<bool>,
    pub supports_multipart_upload: Option<bool>,
    pub supports_multipart_copy: Option<bool>,
    pub supports_presigned_get: Option<bool>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_preset_matrix_has_safe_defaults() {
        assert_eq!(ProviderType::AwsS3.default_region(), "");
        assert_eq!(
            ProviderType::AwsS3.default_addressing_style(),
            AddressingStyle::VirtualHosted
        );
        assert_eq!(ProviderType::AwsS3.endpoint_for("us-east-1", None), None);

        assert_eq!(ProviderType::CloudflareR2.default_region(), "auto");
        assert_eq!(
            ProviderType::CloudflareR2.endpoint_for("auto", Some("acct-123")),
            Some("https://acct-123.r2.cloudflarestorage.com".to_string())
        );
        assert_eq!(
            ProviderType::CloudflareR2.default_addressing_style(),
            AddressingStyle::VirtualHosted
        );

        assert_eq!(ProviderType::Minio.default_region(), "us-east-1");
        assert_eq!(
            ProviderType::Minio.default_addressing_style(),
            AddressingStyle::Path
        );
        assert_eq!(ProviderType::Minio.endpoint_for("us-east-1", None), None);

        assert_eq!(ProviderType::Wasabi.default_region(), "us-east-1");
        assert_eq!(
            ProviderType::Wasabi.endpoint_for("us-east-1", None),
            Some("https://s3.wasabisys.com".to_string())
        );
        assert!(ProviderType::Wasabi
            .endpoint_for("not-a-packaged-region", None)
            .is_none());

        assert_eq!(ProviderType::CustomS3.default_region(), "us-east-1");
        assert_eq!(ProviderType::CustomS3.endpoint_for("us-east-1", None), None);
        assert_eq!(
            ProviderType::CustomS3.default_addressing_style(),
            AddressingStyle::VirtualHosted
        );
    }

    #[test]
    fn wasabi_preset_table_is_versioned_and_complete() {
        let table: WasabiPresetTable =
            serde_json::from_str(include_str!("wasabi-presets.json")).unwrap();
        assert_eq!(table.version, 1);
        assert!(table.regions.len() >= 5);
    }

    #[test]
    fn capability_baseline_is_conservative_for_unknown_deployments() {
        let known = ProviderType::AwsS3.capability_baseline();
        assert_eq!(known.supports_multipart_upload, Some(true));
        assert_eq!(known.supports_presigned_get, Some(true));
        assert_eq!(known.can_list_buckets, None);

        let custom = ProviderType::CustomS3.capability_baseline();
        assert_eq!(
            custom,
            ProviderCapabilityBaseline {
                can_list_buckets: None,
                can_head_bucket: None,
                supports_multipart_upload: None,
                supports_multipart_copy: None,
                supports_presigned_get: None,
            }
        );
    }

    #[test]
    fn provider_identifiers_round_trip_without_aliasing_unknown_values() {
        for provider in [
            ProviderType::AwsS3,
            ProviderType::CloudflareR2,
            ProviderType::Minio,
            ProviderType::Wasabi,
            ProviderType::CustomS3,
        ] {
            assert_eq!(ProviderType::parse_known(provider.as_str()), Some(provider));
            assert_eq!(ProviderType::parse(provider.as_str()), provider);
        }
        assert_eq!(ProviderType::parse_known("unknown"), None);
        assert_eq!(ProviderType::parse("unknown"), ProviderType::CustomS3);
    }
}
