SECURITY POLICY - MNX DataForge

SUPPORTED VERSIONS

Only the latest signed build (the newest GitHub Release, tag build-<N>) receives security fixes.
Older builds are not patched; update from Settings > App update.

REPORTING A VULNERABILITY

Please do not open a public issue for a security problem.

1. Go to the repository, open the Security tab, then "Report a vulnerability" (GitHub private reporting).
2. Include: the build number (Settings shows it), Android version and device, steps to reproduce,
   and what an attacker could do. Logs from Settings > Logs help, but remove any private dataset text first.

WHAT TO EXPECT

- Acknowledgement within 7 days.
- A first assessment (accepted, needs more information, or declined with the reason) within 14 days.
- If accepted: a fix in a new signed build, then a public note in the release description.
  Credit is given if you want it.
- If declined: you get the reason, and you may reply with more evidence.
This project is maintained by one person, so these are targets, not guarantees.

SCOPE

In scope: the Android app, the Rust engine, the in-app updater (download and install flow),
backup and restore files, the log and "send to GitHub" feature, the CI signing and release workflow.
Out of scope: attacks that need a rooted or already compromised device, and issues in Android or
third-party libraries themselves (report those upstream).

DESIGN NOTES

- No telemetry, no accounts. The internet permission is used only when you press "Check for updates"
  or "Download and install".
- Updates are accepted by Android only if signed with the same release key as the installed app.
  The signing key is stored only in GitHub Actions secrets and with the maintainer, never in the repository.
- Logs stay on the device until you copy, save or send them yourself.
