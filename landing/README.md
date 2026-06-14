# MagnetBox site (landing + docs)

Two self-contained pages — `index.html` (landing) and `docs.html` (documentation).
No build step, no external scripts/fonts. Host the **whole folder free** on a static host.

## Before you publish — fill in the placeholders
Search both files for `TODO` and replace:
- **Donation links** (`Ko-fi` / `PayPal`, in `index.html` `#support`) → already wired; update if your handles change.
- **"Live demo"** (nav) → your demo instance URL (or remove the link).
- **GitHub / repo links** (nav + `docs.html` top bar) → your repository URL.
- The `docs.html` link is already wired into the landing nav/footer — no change needed.

## Deploy free — Cloudflare Pages (recommended)
1. Put this `landing/` folder in a Git repo (GitHub/GitLab).
2. cloudflare.com → **Workers & Pages → Create → Pages → Connect to Git**.
3. Build command: *(none)*. Output directory: `landing` (or repo root if you move the files).
4. Deploy → you get a free `*.pages.dev` URL; add your custom domain in **Custom domains**.

## Or GitHub Pages
1. Push to a GitHub repo.
2. Repo **Settings → Pages → Source: Deploy from branch**, pick the branch + the
   folder containing `index.html`.
3. It publishes at `https://<user>.github.io/<repo>/`; add a custom domain if you like.

## Or any static host
Netlify (drag-and-drop the folder), Vercel, or even an S3 bucket — it's just one HTML file.

> The page intentionally avoids external CDNs/fonts so it loads instantly and has
> no third-party dependencies or tracking.
