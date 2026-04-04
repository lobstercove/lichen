# Auto Update Policy

## Browser Store Channel

- Chrome Web Store publication provides automatic updates for Chrome users.
- Microsoft Edge Add-ons publication provides automatic updates for Edge users.

This is the recommended distribution channel for production users.

## Direct ZIP Or Unpacked Channel

- Direct ZIP installs and unpacked developer-mode installs do not provide a consumer-friendly automatic update path.
- These channels are for review, QA, enterprise testing, or temporary manual distribution.

## Release Feed

The packaging workflow emits `latest.json` so the wallet install page can display the current extension version, checksums, and release metadata consistently across the website and GitHub releases.