# Google Drive backup: setup guide

ChatVault can back up, sync and restore chats with each user's **own** Google Drive. Users just click
**Sync & backup → Google Drive → Connect Google Drive** and sign in with Google in their browser. They never create a
Google Cloud project or paste keys. To make that possible, the maintainer registers ChatVault with Google **once**,
as described here.

There is no ChatVault server. Files go straight from the user's PC to the user's Drive, and nothing is stored in the
maintainer's Drive.

---

## 1. How it works (for reviewers and contributors)

| Part | What ChatVault does |
| --- | --- |
| Sign-in | OAuth 2.0 for desktop apps: the system browser opens Google's consent page. Google redirects to a one-time listener on `http://127.0.0.1:<random port>` (loopback redirect). PKCE (`S256`) and a random `state` value protect the exchange. |
| Permission | Only `https://www.googleapis.com/auth/drive.appdata`, Drive's hidden **application data folder**. ChatVault cannot see or change any other file. The account name and email shown in the app come from Drive's `about.get` (`fields=user`), so no `openid`, `email` or `profile` scope is requested. |
| Tokens | The refresh token is kept in the **Windows Credential Manager** (`keyring` crate, entry `ChatVault Google Drive`). The access token stays in memory only. Neither is ever logged, written to a file, or put in an error message. |
| Data | The files of the local backup folder, compressed with zstd. Each PC writes its own copies (marked with a random PC id in `appProperties`), so PCs never overwrite each other. Each PC downloads the others' files. A local file is only replaced when nothing can be lost: either the local copy already matched Drive, or the newer copy is the same chat with more messages. Otherwise both copies are kept (`name.from-<PC>.ext`). A sync never deletes anything. |
| Large files | Files over 5 MB use resumable uploads in 8 MB chunks, so an interrupted upload continues from where it stopped. Failed requests are retried with back-off (connection errors, `429`, `5xx`). |
| Offline | Every Drive call is optional. Offline or failing, ChatVault keeps working and shows the last error in Sync & backup. |
| Disconnect | Revokes the token at `https://oauth2.googleapis.com/revoke` and deletes it from the Credential Manager. Optionally deletes every ChatVault file in the app folder first. |

Code: `src-tauri/src/gdrive.rs` (backend), `src/gdrive-panel.tsx` (UI), `src-tauri/build.rs` (credentials at build
time).

### Why the client secret is not a secret

Google's documentation says that installed (desktop) apps
[cannot keep secrets](https://developers.google.com/identity/protocols/oauth2/native-app), and that for them "the client
secret is obviously not treated as a secret" ([OAuth 2.0 overview](https://developers.google.com/identity/protocols/oauth2)).
Anything built into the `.exe` can be extracted. Security comes from **PKCE**, the **loopback redirect** and the
user's own consent, not from the secret. ChatVault still keeps the values out of the public repository, as
[Google's OAuth policies](https://developers.google.com/identity/protocols/oauth2/policies) ask: they are injected at
build time from GitHub Actions secrets.

---

## 2. Create the Google Cloud project

1. Open the [Google Cloud console](https://console.cloud.google.com/) and sign in with the Google account that will own
   ChatVault's registration. Prefer a dedicated account, or one you will keep for years.
2. [Create a project](https://console.cloud.google.com/projectcreate), for example **ChatVault**. No billing account is
   needed: the Drive API is free (see [section 9](#9-quotas-limits-and-cost)).

## 3. Enable the Google Drive API

1. Open [Google Drive API in the API Library](https://console.cloud.google.com/apis/library/drive.googleapis.com) with
   the new project selected.
2. Click **Enable**.

## 4. Configure the Google Auth Platform (OAuth consent)

Open [Google Auth Platform](https://console.cloud.google.com/auth/overview) and click **Get started**.

1. **App information**
   - App name: `ChatVault`
   - User support email: an address users can write to.
2. **Audience:** **External**, so anyone with a Google account can connect.
3. **Contact information:** your email, for Google's notices.
4. Accept the Google API Services User Data Policy and click **Create**.

### Branding

Open [Branding](https://console.cloud.google.com/auth/branding):

| Field | Value |
| --- | --- |
| App name | `ChatVault` |
| User support email | your support address |
| App logo | `docs/assets/icon-256.png` (120×120 px or larger, square, PNG). Adding a logo requires brand verification. |
| Application home page | `https://ssanaullahrais.github.io/Chat-Vault/` |
| Application privacy policy link | `https://ssanaullahrais.github.io/Chat-Vault/privacy.html` |
| Application terms of service link | `https://ssanaullahrais.github.io/Chat-Vault/terms.html` |
| Authorized domains | `ssanaullahrais.github.io` |
| Developer contact information | your email |

> **Authorized domain on GitHub Pages:** Google asks you to prove that you own each authorized domain in
> [Google Search Console](https://search.google.com/search-console). Add the URL-prefix property
> `https://ssanaullahrais.github.io/Chat-Vault/` and verify it with the **HTML file** method: download the
> `google<code>.html` file, put it in `docs/`, push, then click **Verify**. Keep the file in `docs/` afterwards.
> (`github.io` is a shared domain, so if Google will not accept it, a custom domain pointed at GitHub Pages is the
> alternative. It is optional and not needed to start.)

### Data access (scopes)

Open [Data Access](https://console.cloud.google.com/auth/scopes) → **Add or remove scopes**, and add only:

```
https://www.googleapis.com/auth/drive.appdata
```

Google lists it as **non-sensitive**
([Drive scopes](https://developers.google.com/workspace/drive/api/guides/api-specific-auth)), so no sensitive-scope
review and no security assessment (CASA) is needed. Do **not** add `drive`, `drive.file` or `drive.readonly`. They
are not needed, and the broader ones are restricted scopes.

### Test users (while in Testing)

Open [Audience](https://console.cloud.google.com/auth/audience). While the publishing status is **Testing**, only
the accounts listed under **Test users** can sign in. Add your own and any testers (up to 100).

In Testing, Google issues refresh tokens that **expire after 7 days**. Testers then see "Google Drive access was
removed or has expired. Connect Google Drive again." That is expected and goes away in production.

## 5. Create the Desktop OAuth client

1. Open [Clients](https://console.cloud.google.com/auth/clients) → **Create client**.
2. Application type: **Desktop app**. Name: `ChatVault desktop`.
3. Click **Create**, then **Download JSON**. The file is named `client_secret_<id>.apps.googleusercontent.com.json`.
   It holds `client_id` and `client_secret`.
4. Keep this file private. Never commit it (`.gitignore` excludes `client_secret*.json` and `.env`).

Desktop clients need no redirect URI setting: Google accepts any `http://127.0.0.1:<port>` loopback redirect for them.

## 6. Add the credentials to GitHub Actions

Official releases are built by `.github/workflows/release.yml`, which passes these secrets to the build. There,
`src-tauri/build.rs` bakes them into the app.

1. Open the repository's **Settings → Secrets and variables → Actions**
   (`https://github.com/ssanaullahrais/Chat-Vault/settings/secrets/actions`).
2. **New repository secret**:
   - `GOOGLE_CLIENT_ID`: the `client_id` value
   - `GOOGLE_CLIENT_SECRET`: the `client_secret` value
3. Release as usual by pushing a `v*` tag. If the secrets are missing, the build still succeeds and that build simply
   has no Google Drive.

GitHub masks secret values in logs. `build.rs` never prints them. Do not run builds with `cargo -vv` in CI, because
very verbose mode can print build-script directives.

## 7. Build from source with your own credentials (contributors)

1. Do sections 2 to 5 in **your own** Google Cloud project (Testing status is fine, with yourself as test user).
2. Copy `.env.example` to `.env` next to `package.json` and fill in:
   ```
   GOOGLE_CLIENT_ID=1234567890-abc.apps.googleusercontent.com
   GOOGLE_CLIENT_SECRET=GOCSPX-...
   ```
3. Run `npm run tauri dev` or `npm run tauri build`. Environment variables of the same names override `.env`.
   After changing `.env`, the next build picks it up automatically.

Without `.env`, ChatVault builds and runs normally. Sync & backup → Google Drive then says it is not available in
that build.

## 8. Test the integration

**Locally (dev build)**
1. Create `.env` as above and run `npm run tauri dev`.
2. Open **Sync & backup → Google Drive → Connect Google Drive**, sign in with a test user and allow access. The
   browser shows "ChatVault is connected", and the app shows the account.
3. Click **Sync now**. The progress bar shows uploading, and "Last synced" updates.
4. Check that the files reached Drive: open Google Drive → ⚙ **Settings → Manage apps → ChatVault**. The hidden app
   data size grows.
5. On a second PC (or a second Windows user), connect the same account and click **Sync now**. Chats from the first PC
   appear in the list. **Restore from Google Drive** also copies missing chats back into the tools.
6. Click **Disconnect** with **Also delete the backup from Google Drive** on. The app shows "not connected", and
   Manage apps shows no hidden data.
7. Unit tests: `cd src-tauri && cargo test --lib gdrive::` (PKCE, state check, path safety, merge rules).

**Release build**
1. Add the two secrets, set the version in the package and Tauri configuration before pushing the matching release tag.
2. Install the built setup on a clean PC and repeat steps 2 to 6.

## 9. Publish for everyone

1. Open [Audience](https://console.cloud.google.com/auth/audience) → **Publish app** → confirm. The status becomes
   **In production**.
2. Because the app only uses a non-sensitive scope, it needs **brand verification only**. That covers the name, logo,
   home page, privacy policy and authorized domain. It is usually quick, and is started from
   [Verification Center](https://console.cloud.google.com/auth/verification). Until brand verification finishes, users
   see an "unverified app" notice, and the logo is not shown.
3. Requirements Google checks
   ([verification requirements](https://support.google.com/cloud/answer/13464321)):
   - The home page describes ChatVault, is publicly reachable, and links to the privacy policy.
   - The privacy policy is on the same domain as the home page, and describes how Google user data is accessed, used,
     stored and shared, including the Limited Use statement (see `docs/privacy.html`, sections 7 and 8).
   - The app name and logo match the home page.
   - The authorized domain is verified in Search Console.
4. Respond to any email from Google's review team from the developer contact address.

## 10. Quotas, limits and cost

- The **Google Drive API is free**. There is no charge per request, and no billing account is needed.
- Default quotas are generous for this use (on the order of 12,000 requests per minute per project, and lower per
  user). ChatVault only uploads changed files and retries politely on `429`. Check usage in
  [Quotas](https://console.cloud.google.com/apis/api/drive.googleapis.com/quotas).
- The backup counts toward **each user's own** Google storage (15 GB free per account). The maintainer pays nothing
  and stores nothing.
- Each Google account can hold up to **100 refresh tokens per client**. Beyond that, the oldest one stops working
  silently. With one connection per PC this is not a concern.
- Google may delete OAuth clients unused for 6 months. A deleted client can be restored within 30 days.

## 11. Useful links

- [OAuth 2.0 for iOS & desktop apps (loopback, PKCE)](https://developers.google.com/identity/protocols/oauth2/native-app)
- [Store application-specific data (appDataFolder)](https://developers.google.com/workspace/drive/api/guides/appdata)
- [Choose Google Drive API scopes](https://developers.google.com/workspace/drive/api/guides/api-specific-auth)
- [Upload file data (resumable uploads)](https://developers.google.com/workspace/drive/api/guides/manage-uploads)
- [OAuth 2.0 policies](https://developers.google.com/identity/protocols/oauth2/policies)
- [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy)
- [App verification requirements](https://support.google.com/cloud/answer/13464321)
