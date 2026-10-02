# Hosted Studio

Spiral Robotics Studio for Spiral colleagues at `robotics.spiraldb.com`,
behind the same Cloudflare Access sign-in as `next.spiraldb.com`.

## Architecture

- One Compute Engine VM in its own Google Cloud project, `spiraldb-robotics`, runs two containers with Docker Compose: Studio and `cloudflared`.
- A Cloudflare Tunnel carries all traffic. The VM publishes no ports and admits only IAP SSH.
- Cloudflare Access admits only Spiral team identities and provides HTTPS.
- Studio keeps its state in one Docker volume: its SQLite database and its Spiral sign-in.
- Each release, a pushed `v*` tag, builds the image and deploys it to the VM.
- The heavy work runs elsewhere: data lives in SpiralDB, model calls go to OpenRouter, and Score progress runs on Modal. The VM needs no GPU.

| Path | Purpose |
| --- | --- |
| `Dockerfile` | The image: backend, web interface, Spiral driver with its video runtime |
| `compose.yaml` | Studio and `cloudflared` on the VM |
| `deploy.sh` | Runs on the VM: installs `compose.yaml` and runs one image |
| `../../.github/workflows/hosted-image.yml` | Builds and publishes the image, then deploys it |
| `robotics.tf` in `spiraldb/infra` | The VM, its network and its snapshot schedule |

## What colleagues share

Studio was built for one person on one machine. Hosted, everyone who passes Access shares it.

- One Spiral sign-in. Everyone reads and writes SpiralDB as that account. **Sign out** or a new organization in Studio changes it for everyone.
- One SpiralDB project. Switching it in **Settings → SpiralDB** switches it for everyone.
- One set of provider keys and settings.
- Curation drafts. When two people edit the same draft, the last save wins.
- Live and Setup show no robots or cameras. The VM has none.

## Image

The `Hosted image` workflow publishes `ghcr.io/spiraldb/spiral-robotics-studio` for each release.
It tags each image with its release, such as `v0.1.0`, and `latest`.
The image ships a Spiral driver built from SpiralDB `develop`, with its video runtime and LGPL FFmpeg.
The `com.spiraldb.spiraldb.revision` label names the SpiralDB commit.

The package is private, like this repository.
The deploy job pulls it on the VM with the job's own token.
Pulling it anywhere else needs a classic GitHub personal access token with the `read:packages` scope.
If the spiraldb organization enforces single sign-on, authorize the token for it.

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
  --project=spiraldb-robotics
```

Delete the default network, whose rules admit SSH and RDP from anywhere.
Terraform creates the VM's network.
Forbid service account keys: the deployer signs in through Workload Identity Federation only.

```bash
gcloud compute firewall-rules delete default-allow-icmp default-allow-internal \
  default-allow-rdp default-allow-ssh --project=spiraldb-robotics --quiet
gcloud compute networks delete default --project=spiraldb-robotics --quiet
gcloud resource-manager org-policies enable-enforce \
  iam.disableServiceAccountKeyCreation --project=spiraldb-robotics
```

Create the deployer account. Only this repository's workflows can act as it:

```bash
gcloud iam service-accounts create studio-deployer --project=spiraldb-robotics \
  --display-name="Studio deployer"
gcloud iam service-accounts add-iam-policy-binding \
  studio-deployer@spiraldb-robotics.iam.gserviceaccount.com --project=spiraldb-robotics \
  --role=roles/iam.workloadIdentityUser \
  --member="principalSet://iam.googleapis.com/projects/733253205867/locations/global/workloadIdentityPools/github-actions/attribute.repository/spiraldb/spiral-robotics"
```

Let the deployer and the administrators SSH to the VM through IAP, with sudo:

```bash
for MEMBER in serviceAccount:studio-deployer@spiraldb-robotics.iam.gserviceaccount.com \
  user:xiangpeng@spiraldb.com; do
  for ROLE in roles/compute.osAdminLogin roles/iap.tunnelResourceAccessor roles/compute.viewer; do
    gcloud projects add-iam-policy-binding spiraldb-robotics \
      --member="$MEMBER" --role="$ROLE" --condition=None
  done
done
```

Let Terraform CI in `spiraldb/infra` manage the project's Compute Engine resources:

```bash
gcloud projects add-iam-policy-binding spiraldb-robotics \
  --member=serviceAccount:terraform-ci@pyspiral-dev.iam.gserviceaccount.com \
  --role=roles/compute.admin --condition=None
```

### 2. Create the VM (`spiraldb/infra`)

`robotics.tf` in `spiraldb/infra` owns the VPC, the IAP SSH firewall rule, the VM and its snapshot schedule.
Change them there through a pull request; CI applies it after approval.

- `spiral-robotics-studio` in `us-east4-a`: `e2-standard-4`, Debian 13, a 100 GB `pd-balanced` boot disk.
- `us-east4` is SpiralDB's US region. Every Studio query makes several round trips to SpiralDB.
- Studio calls no Google Cloud API, so the VM has no service account.
- An external IP serves egress. The VPC admits nothing but IAP SSH.
- OS Login is on. A startup script installs Docker and Compose from Debian.

The boot disk holds the only copy of Studio's drafts and settings.
It is snapshotted daily at 08:00 UTC, and each snapshot is kept for 14 days.
Terraform refuses to destroy the VM.

### 3. Create the Access application

Create a Cloudflare Access self-hosted application before the tunnel routes any traffic.

| Setting | Value |
| --- | --- |
| Application name | `Spiral Robotics Studio` |
| Application domain | `robotics.spiraldb.com` |
| Session duration | `24 hours` |

Attach the same allow policy that protects `next.spiraldb.com`.

Do not route the hostname before this application exists.
Studio has no sign-in of its own: anyone who reaches it uses the shared Spiral account.

### 4. Create the tunnel

In Cloudflare Zero Trust, create a `cloudflared` tunnel named `spiral-robotics-studio`.
Copy its token.
Add one public hostname:

| Setting | Value |
| --- | --- |
| Hostname | `robotics.spiraldb.com` |
| Service type | `HTTP` |
| Service URL | `studio:8080` |

`studio` is the Compose service name. `cloudflared` reaches it on the Compose network.

Save the token as a secret of this repository. The deploy job writes it to the VM.
`gh` prompts for the value:

```bash
gh secret set CLOUDFLARE_TUNNEL_TOKEN --repo spiraldb/spiral-robotics
```

### 5. Deploy

Release `v0.1.0` as **Operations** describes.

### 6. Sign in once

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

A release deploys Studio. Tag a commit on `main` with the next version and push the tag:

```bash
git fetch origin
git tag v0.1.1 origin/main
git push origin v0.1.1
```

The tag starts the `Hosted image` workflow, which builds the image and deploys it to the VM.
A deploy restarts Studio, which stops curation runs in progress. Release when nobody is running one.

To run an earlier release again, re-run the **Deploy** job of that release's `Hosted image` run.

Connect to the VM:

```bash
gcloud compute ssh spiral-robotics-studio --project=spiraldb-robotics --zone=us-east4-a \
  --tunnel-through-iap
```

Read the logs:

```bash
cd /opt/spiral-robotics-studio
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
