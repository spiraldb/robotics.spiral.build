# Hosted Studio

Spiral Robotics Studio for Spiral colleagues at `robotics.spiraldb.com`,
behind the same Cloudflare Access sign-in as `next.spiraldb.com`.

## Architecture

- One Compute Engine VM runs two containers with Docker Compose: Studio and `cloudflared`.
- A Cloudflare Tunnel carries all traffic. The VM publishes no ports.
- Cloudflare Access admits only Spiral team identities and provides HTTPS.
- Studio keeps its state in one Docker volume: its SQLite database and its Spiral sign-in.
- The heavy work runs elsewhere: data lives in SpiralDB, model calls go to OpenRouter, and Score progress runs on Modal. The VM needs no GPU.

| Path | Purpose |
| --- | --- |
| `Dockerfile` | The image: backend, web interface, Spiral driver with its video runtime |
| `compose.yaml` | Studio and `cloudflared` on the VM |
| `../../.github/workflows/hosted-image.yml` | Builds and publishes the image |

## What colleagues share

Studio was built for one person on one machine. Hosted, everyone who passes Access shares it.

- One Spiral sign-in. Everyone reads and writes SpiralDB as that account. **Sign out** or a new organization in Studio changes it for everyone.
- One SpiralDB project. Switching it in **Settings → SpiralDB** switches it for everyone.
- One set of provider keys and settings.
- Curation drafts. When two people edit the same draft, the last save wins.
- Live and Setup show no robots or cameras. The VM has none.

## Image

The `Hosted image` workflow publishes `ghcr.io/spiraldb/spiral-robotics-studio` on every push to `main`.
It tags each image `latest` and with the Studio commit.
The image ships a Spiral driver built from SpiralDB `develop`, with its video runtime and LGPL FFmpeg.
The `com.spiraldb.spiraldb.revision` label names the SpiralDB commit.

The package is private, like this repository.
Pulling it needs a GitHub token with the `read:packages` scope.

## Production setup

### Create the Access application

Create a Cloudflare Access self-hosted application before the tunnel routes any traffic.

| Setting | Value |
| --- | --- |
| Application name | `Spiral Robotics Studio` |
| Application domain | `robotics.spiraldb.com` |
| Session duration | `24 hours` |

Attach the same allow policy that protects `next.spiraldb.com`.

Do not route the hostname before this application exists.
Studio has no sign-in of its own: anyone who reaches it uses the shared Spiral account.

### Create the tunnel

In Cloudflare Zero Trust, create a `cloudflared` tunnel named `spiral-robotics-studio`.
Copy its token.
Add one public hostname:

| Setting | Value |
| --- | --- |
| Hostname | `robotics.spiraldb.com` |
| Service type | `HTTP` |
| Service URL | `studio:8080` |

`studio` is the Compose service name. `cloudflared` reaches it on the Compose network.

### Create the VM

```bash
gcloud compute instances create spiral-robotics-studio \
  --project=PROJECT \
  --zone=us-east4-a \
  --machine-type=e2-standard-4 \
  --image-family=debian-13 \
  --image-project=debian-cloud \
  --boot-disk-size=100GB \
  --boot-disk-type=pd-balanced \
  --no-service-account \
  --no-scopes
```

- `us-east4` is SpiralDB's US region. Every Studio query makes several round trips to SpiralDB.
- Studio calls no Google Cloud API, so the VM has no service account.
- The VM needs outbound internet access: an external IP, or Cloud NAT.
- Add no ingress firewall rule. SSH is the only way in that an administrator needs.

The disk holds the only copy of Studio's drafts and settings.
Attach a daily snapshot schedule:

```bash
gcloud compute resource-policies create snapshot-schedule spiral-robotics-studio-daily \
  --project=PROJECT \
  --region=us-east4 \
  --daily-schedule \
  --start-time=08:00 \
  --max-retention-days=14

gcloud compute disks add-resource-policies spiral-robotics-studio \
  --project=PROJECT \
  --zone=us-east4-a \
  --resource-policies=spiral-robotics-studio-daily
```

### Start Studio

Install Docker Engine on the VM with Docker's instructions for Debian.

Sign in to the registry with the GitHub token:

```bash
sudo docker login ghcr.io --username GITHUB_USER
```

Copy `compose.yaml` to `/opt/spiral-robotics-studio/` on the VM.
Beside it, create `.env` with the tunnel token:

```bash
TUNNEL_TOKEN=TOKEN_FROM_CLOUDFLARE
```

Start both containers:

```bash
cd /opt/spiral-robotics-studio
sudo docker compose up -d
```

### Sign in once

1. Open `https://robotics.spiraldb.com` and pass Access. Studio opens **Settings → SpiralDB**.
2. Choose **Sign in with Spiral**. Confirm the code with the account everyone will share.
3. Pick the project.
4. In **Settings → Keys**, save the keys the team uses:

| Key | Used by |
| --- | --- |
| OpenRouter API key | Break down, Classify, Find similar |
| Modal base URL and Modal token | Score progress |
| Hugging Face token | Downloads of models and datasets from Hugging Face |

The volume keeps the sign-in and the keys across restarts and updates.

## Operations

Update to the newest image:

```bash
cd /opt/spiral-robotics-studio
sudo docker compose pull
sudo docker compose up -d
```

A restart stops curation runs in progress. Update when nobody is running one.

To pin a version, replace `latest` in `compose.yaml` with a Studio commit.

Read the logs:

```bash
sudo docker compose logs --follow studio
```

## Limits

- Cloudflare rejects request bodies over 100 MB. Project bundle imports above that fail, though Studio accepts up to 256 MB.
- Cloudflare ends a request when Studio takes more than 100 seconds to start its response.
- When the Access session expires, reload the page.

## Build the image locally

Build the web interface in its own target directory, so a running `dx serve` is never touched:

```bash
rm -rf target/hosted/web target/hosted/dx/spiral-robotics-frontend/release/web/public
CARGO_TARGET_DIR=target/hosted nix develop .#web --command dx bundle --web --release \
  --debug-symbols false --package spiral-robotics-frontend --out-dir target/hosted/web
```

Then build the image with a SpiralDB checkout:

```bash
docker buildx build --platform linux/amd64 \
  --file deploy/hosted/Dockerfile \
  --build-context spiraldb=../spiraldb \
  --build-context web=target/hosted/web/public \
  --tag spiral-robotics-studio .
```
