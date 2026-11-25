# Changelog

What changed in each version of ChatVault. The newest version is at the top. Downloads are on the
[Releases page](https://github.com/ssanaullahrais/Chat-Vault/releases).

## [0.6.0] - 2026-11-25

### New

- **Google Drive backup and sync (optional).** Connect your Google account in *Sync & backup → Google Drive* and your
  backup (the chats and their pictures, compressed) is copied to your own Google Drive, straight from your PC. Use the
  same account on your other PCs to see and restore their chats there too.
  - ChatVault asks Google for one permission only: its own hidden folder in your Drive. It cannot see or change your
    other files, and there is no ChatVault server in between.
  - Your sign-in is kept in the Windows Credential Manager. Disconnect at any time; your backup stays in Drive.
  - Syncing never deletes anything. If a chat changed on two PCs, both copies are kept.
  - Large files and interrupted syncs carry on where they stopped, even after ChatVault was closed.
  - **Cancel** pauses a sync, restore or download: what was already done stays, and the next sync goes on from there.
  - ChatVault checks that the connection still works and tells you if its access was removed in your Google account.
- **Backups tab.** One list of your backups: the backup folder on this PC, and each PC's backup in Google Drive, with
  how many chats it holds, its size, when it was last updated and whether anything is waiting to upload, download or
  restore. From each backup you can back up now, sync, restore missing chats, download another PC's backup, or delete
  a backup (it asks you to type CONFIRM, and warns you when a chat exists nowhere else). *View chats* lists the chats
  of one backup, with search and filters.
- **Chats look like they do in the tools.** Each turn shows a short summary of the steps the AI took (files read,
  edits with their added and removed lines, commands, searches), its plan at the end of the turn, question and
  wrap-up cards, notices, and messages you sent while it was still working. Pictures sent with a message are shown with
  it. This works for Claude Code, Codex, Cursor, Gemini CLI, OpenCode and Cline.
- **Better search.** Results are ranked by how well they match, and the matching words are highlighted, in the sidebar
  and in *Search all chats*.
- **Live updates.** New chats, new messages and renamed chats appear right away, without refreshing.
- **OpenCode 2.** Chats from OpenCode 2.x are read again.
- **Backups include pictures and tool output:** Claude Code's tool results and helper agents, Codex's generated pictures
  and attachments, and Gemini CLI's tool output.
- **A confirmation before every action** in *Sync & backup*, so nothing runs by accident.

### Improved

- ChatVault runs as one window: opening it again brings the open window to the front.
- The loading screen shows from the very start, and you can move the window while your chats are loading.
- *Sync & backup* is reorganised: *Backups* comes first, then *Backup folder*, *Automatic* and *Google Drive*; *Restore*
  and *Export* are grouped under *Get chats back*.
- A confirmation now hides the window it was opened from, so only one box is on screen.
- Your own messages are narrower, like in the tools, and shorter labels in menus.
- The privacy policy and terms inside the app describe Google Drive backup.

### Fixed

- Searching in the sidebar found too little, or the wrong chats.
- Renamed chats kept their old name until ChatVault was restarted.
- Some steps and messages in Codex, Gemini CLI and OpenCode chats were not shown.

## Earlier versions

See the [Releases page](https://github.com/ssanaullahrais/Chat-Vault/releases) for 0.5.0 and earlier.
