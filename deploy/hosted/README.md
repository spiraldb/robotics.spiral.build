# Hosted Studio

Spiral Robotics Studio for Spiral colleagues at `robotics.spiraldb.com`,
behind the same Cloudflare Access sign-in as `next.spiraldb.com`.

## Architecture

- One Compute Engine VM in its own Google Cloud project, `spiraldb-robotics`, runs two containers with Docker Compose: Studio and `cloudflared`.
- A Cloudflare Tunnel carries all traffic. The VM publishes no ports and admits only IAP SSH.
- Cloudflare Access admits only Spiral team identities and provides HTTPS.
- Every colleague signs in to Spiral in Studio too, and Studio reaches Spiral as them.
- Studio keeps its state in one Docker volume: its SQLite database, which holds each browser's Spiral session.
- Like SpiralDB's Cloud Run services, Terraform owns the spec and CI owns the image. Each release, a pushed `v*` tag, publishes the image as `latest`, and the VM pulls it within two minutes. CI has no access to the VM.
- The heavy work runs elsewhere: data lives in SpiralDB, model calls go to OpenRouter, and Score progress runs on Modal. The VM needs no GPU.
- The provider keys are deployment secrets in Secret Manager. Studio reads them from its environment; no colleague sees or sets them.

| Path | Purpose |
| --- | --- |
| `hosted-image` in `../../flake.nix` | The image: backend, web interface, Spiral driver with its video runtime and FFmpeg |
| `../../.github/workflows/hosted-image.yml` | Builds the image and pushes it to Artifact Registry |
| `robotics.tf` in `spiraldb/infra` | The VM, its network, its snapshot schedule and its image access |
| `config/robotics-studio/` in `spiraldb/infra` | The Compose spec and the startup script that keeps the VM on it |

## What colleagues share

Each colleague signs in with their own Spiral account. They see the organizations and projects their account can open, and their queries, recordings and curation runs reach Spiral as them. They still share:

- The deployment's provider keys. For now Spiral pays for everyone's model calls and Score progress runs.
- Studio's records of a project they can all open, such as curation drafts and published views. When two people edit the same draft, the last save wins.

Robots are not part of the hosted Studio. Rollout, including its robot setup, talks to the robot connector next to the arms, at the address saved in the workspace; `localhost` means each colleague's own machine.

## Image

The `Hosted image` workflow publishes `us-docker.pkg.dev/pyspiral-dev/spiral-docker/spiral-robotics-studio`, beside SpiralDB's own images.
It publishes one image per release, tagged with the release, such as `v0.1.0`, and `latest`, the tag the VM runs.
The image ships the Spiral driver release that `flake.nix` pins, with its video runtime and the FFmpeg 9 libraries from nixpkgs.
The `com.spiraldb.adbc.version` label names the driver release.

To pull it elsewhere, run `gcloud auth configure-docker us-docker.pkg.dev` with an account that can read `spiral-docker`.

## Production setup

### 1. Bootstrap the project (by hand, once)

Put the project beside `spiraldb-auth` and bill it to the same account:

```bash
gcloud projects describe spiraldb-auth --format='value(parent.type,parent.id)'
gcloud billing projects describe spiraldb-auth --format='value(billingAccountName)'
```

```bash
gcloud projects create spiraldb-robotics --folder=FOLDER_ID
gcloud billing projects link spiraldb-robotics --billing-account=BILLING_ACCOUNT_ID
gcloud services enable compute.googleapis.com iap.googleapis.com oslogin.googleapis.com \
  secretmanager.googleapis.com --project=spiraldb-robotics
```

Delete the default network, whose rules admit SSH and RDP from anywhere.
Terraform creates the VM's network.
Forbid service account keys: the VM's account uses Google's managed credentials only.

```bash
gcloud compute firewall-rules delete default-allow-icmp default-allow-internal \
  default-allow-rdp default-allow-ssh --project=spiraldb-robotics --quiet
gcloud compute networks delete default --project=spiraldb-robotics --quiet
gcloud resource-manager org-policies enable-enforce \
  iam.disableServiceAccountKeyCreation --project=spiraldb-robotics
```

Create the VM's service account.
It reads the tunnel token and pulls the image; `robotics.tf` grants the image access.

```bash
gcloud iam service-accounts create spiral-robotics-studio --project=spiraldb-robotics \
  --display-name="Spiral Robotics Studio VM"
```

Let Terraform CI in `spiraldb/infra` manage the project's Compute Engine resources and attach the account to the VM:

```bash
for ROLE in roles/compute.admin roles/iam.serviceAccountViewer; do
  gcloud projects add-iam-policy-binding spiraldb-robotics \
    --member=serviceAccount:terraform-ci@pyspiral-dev.iam.gserviceaccount.com \
    --role="$ROLE" --condition=None
done
gcloud iam service-accounts add-iam-policy-binding \
  spiral-robotics-studio@spiraldb-robotics.iam.gserviceaccount.com --project=spiraldb-robotics \
  --member=serviceAccount:terraform-ci@pyspiral-dev.iam.gserviceaccount.com \
  --role=roles/iam.serviceAccountUser
```

Let administrators SSH to the VM through IAP, with sudo.
OS Login also requires `serviceAccountUser` on the VM's account:

```bash
ADMIN=user:xiangpeng@spiraldb.com
for ROLE in roles/compute.osAdminLogin roles/iap.tunnelResourceAccessor roles/compute.viewer; do
  gcloud projects add-iam-policy-binding spiraldb-robotics \
    --member="$ADMIN" --role="$ROLE" --condition=None
done
gcloud iam service-accounts add-iam-policy-binding \
  spiral-robotics-studio@spiraldb-robotics.iam.gserviceaccount.com --project=spiraldb-robotics \
  --member="$ADMIN" --role=roles/iam.serviceAccountUser
```

### 2. Create the Access application

Create a Cloudflare Access self-hosted application before the tunnel routes any traffic.

| Setting | Value |
| --- | --- |
| Application name | `Spiral Robotics Studio` |
| Application domain | `robotics.spiraldb.com` |
| Session duration | `24 hours` |

Attach the same allow policy that protects `next.spiraldb.com`.

Do not route the hostname before this application exists.
Behind Access, Studio asks every browser to sign in to Spiral as well.

### 3. Create the tunnel

In Cloudflare Zero Trust, create a `cloudflared` tunnel named `spiral-robotics-studio`.
Copy its token.
Add one public hostname:

| Setting | Value |
| --- | --- |
| Hostname | `robotics.spiraldb.com` |
| Service type | `HTTP` |
| Service URL | `studio:8080` |

`studio` is the Compose service name. `cloudflared` reaches it on the Compose network.

Store the token in Secret Manager and let the VM's account read it.
Paste the token at the silent prompt, so it stays out of shell history:

```bash
gcloud secrets create cloudflare-tunnel-token --project=spiraldb-robotics \
  --replication-policy=automatic
read -rs TUNNEL_TOKEN && printf '%s' "$TUNNEL_TOKEN" \
  | gcloud secrets versions add cloudflare-tunnel-token --project=spiraldb-robotics --data-file=- \
  && unset TUNNEL_TOKEN
gcloud secrets add-iam-policy-binding cloudflare-tunnel-token --project=spiraldb-robotics \
  --member=serviceAccount:spiral-robotics-studio@spiraldb-robotics.iam.gserviceaccount.com \
  --role=roles/secretmanager.secretAccessor
```

### 4. Store the provider keys

| Secret | Studio variable | Used by |
| --- | --- | --- |
| `openrouter-api-key` | `OPENROUTER_API_KEY` | Break down, Classify, Find similar |
| `huggingface-token` | `HF_TOKEN` | Downloads of models and datasets from Hugging Face |
| `warp-progress-token` | `WARP_PROGRESS_TOKEN` | Score progress, on the Modal service the image names in `WARP_PROGRESS_URL` |

Create each secret, paste its value at the silent prompt, and let the VM's account read it:

```bash
for SECRET in openrouter-api-key huggingface-token warp-progress-token; do
  gcloud secrets create "$SECRET" --project=spiraldb-robotics --replication-policy=automatic
  printf '%s: ' "$SECRET" && read -rs VALUE && echo && printf '%s' "$VALUE" \
    | gcloud secrets versions add "$SECRET" --project=spiraldb-robotics --data-file=- \
    && unset VALUE
  gcloud secrets add-iam-policy-binding "$SECRET" --project=spiraldb-robotics \
    --member=serviceAccount:spiral-robotics-studio@spiraldb-robotics.iam.gserviceaccount.com \
    --role=roles/secretmanager.secretAccessor
done
```

Studio's startup log names the keys it found, never their values.

### 5. Create the VM (`spiraldb/infra`)

`robotics.tf` in `spiraldb/infra` owns the VPC, the IAP SSH firewall rule, the VM, its snapshot schedule and its read access to `spiral-docker`.
`config/robotics-studio/` beside it holds the Compose spec and the startup script.
Change them there through a pull request; CI applies it after approval.

- `spiral-robotics-studio` in `us-east4-a`: `e2-standard-4`, Debian 13, a 100 GB `pd-balanced` boot disk.
- `us-east4` is SpiralDB's US region. Every Studio query makes several round trips to SpiralDB.
- An external IP serves egress. The VPC admits nothing but IAP SSH.
- On every boot, the startup script installs Docker and Compose from Debian and the `studio-update` timer.
- Every two minutes, `studio-update` writes the Compose spec from instance metadata, and the tunnel token and provider keys from Secret Manager into `.env`. Then it pulls `latest` and runs `docker compose up`.

The boot disk holds the only copy of Studio's drafts and settings.
It is snapshotted daily at 08:00 UTC, and each snapshot is kept for 14 days.
Terraform refuses to destroy the VM.

### 6. Register Studio's sign-in callback

Spiral's sign-in returns browsers only to addresses registered for its WorkOS client.
A WorkOS administrator adds this redirect URI to Spiral's client, `client_01J1CRS9MGF103FKTKZDCNJVM3`, beside `https://app.spiraldb.com/callback`:

```text
https://robotics.spiraldb.com/callback
```

The image already tells Studio its address: it sets `SPIRAL_ROBOTICS_PUBLIC_URL=https://robotics.spiraldb.com`.

### 7. Release

Release `v0.1.0` as **Operations** describes.
The VM starts Studio within two minutes of the image.

## Operations

A release deploys Studio. Tag a commit on `main` with the next version and push the tag:

```bash
git fetch origin
git tag v0.1.1 origin/main
git push origin v0.1.1
```

The tag starts the `Hosted image` workflow, which publishes the image as `latest`.
Within two minutes the VM pulls it and restarts Studio, which stops curation runs in progress. Release when nobody is running one.

To run an earlier release again, point `latest` at its image. The next release moves it again:

```bash
IMAGE=us-docker.pkg.dev/pyspiral-dev/spiral-docker/spiral-robotics-studio
gcloud artifacts docker tags add "$IMAGE:v0.1.0" "$IMAGE:latest"
```

To rotate the tunnel token or a provider key, add a secret version. The next update picks it up; a new provider key restarts Studio.
To change the Compose spec, edit `config/robotics-studio/compose.yaml` in `spiraldb/infra`.

Connect to the VM:

```bash
gcloud compute ssh spiral-robotics-studio --project=spiraldb-robotics --zone=us-east4-a \
  --tunnel-through-iap
```

Read the updates and Studio's logs:

```bash
sudo journalctl --unit=studio-update --since=-1h
cd /opt/spiral-robotics-studio && sudo docker compose logs --follow studio
```

## Limits

- Cloudflare rejects request bodies over 100 MB. Project bundle imports above that fail, though Studio accepts up to 256 MB.
- Cloudflare ends a request when Studio takes more than 100 seconds to start its response.
- When the Access session expires, reload the page.

## Build the image locally

The image builds on x86_64 Linux, or on a Linux builder.
The result is a script that streams the image; load it into Docker:

```bash
nix build .#packages.x86_64-linux.hosted-image
./result | docker image load
```
