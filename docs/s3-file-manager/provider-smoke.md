# Provider smoke tests

The release smoke harness lives in `scripts/provider-smoke.mjs`. It is
credential-free by default and never stores credentials or endpoints in the
repository.

## Configuration

Set these values in the local shell or the protected CI environment:

| Variable | Required | Meaning |
| --- | --- | --- |
| `S3FM_SMOKE_PROVIDER` | yes | `awsS3`, `cloudflareR2`, or `minio` |
| `S3FM_SMOKE_REGION` | yes | AWS region, `auto` for R2, or MinIO region |
| `S3FM_SMOKE_ENDPOINT` | R2/MinIO | The configured HTTPS endpoint; AWS uses its SDK endpoint |
| `S3FM_SMOKE_BUCKET` | yes | An existing bucket dedicated to smoke tests |
| `S3FM_SMOKE_ACCESS_KEY_ID` | `--run` | Access key supplied by the protected runner |
| `S3FM_SMOKE_SECRET_ACCESS_KEY` | `--run` | Secret supplied by the protected runner |
| `S3FM_SMOKE_SESSION_TOKEN` | optional | Temporary-session token |
| `S3FM_SMOKE_ALLOW_INSECURE_HTTP` | MinIO HTTP only | Must be `true` for an explicitly approved local HTTP endpoint |
| `S3FM_SMOKE_ALLOW_WRITE` | `--write` | Must be `true` before the harness writes/deletes its own probe object |

The endpoint and bucket must be real values supplied at execution time. Do
not commit them to source, documentation, workflow files, or issue comments.

## Commands

```text
node scripts/provider-smoke.mjs --check
node scripts/provider-smoke.mjs --run
node scripts/provider-smoke.mjs --run --write
```

`--check` validates the provider matrix and URL/region shape without needing
credentials. `--run` exercises `ListBuckets` (while tolerating an access
denial), then the configured bucket through `HeadBucket`, covering the
ListBuckets-denied fallback in AC-PRO-009. `--write` additionally performs a
unique Put/Get/Delete probe and cleans up its own object. It never writes or
deletes any other key.

The manual GitHub Actions workflow
`.github/workflows/provider-smoke.yml` maps repository/environment secrets to
these variables. It is intentionally not part of the normal pull-request
workflow because real provider credentials and buckets are required.
