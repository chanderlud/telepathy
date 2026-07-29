# Privacy Policy

Telepathy is a peer-to-peer voice, text, attachment, and screen-sharing application built with Flutter, Rust, and [iroh](https://docs.iroh.computer/). This policy covers the open-source application and the official web deployment at [telepathy.chanchan.dev](https://telepathy.chanchan.dev/). Other deployments and distribution services may handle data under their own policies.

## Communications

Communication content is end-to-end encrypted in transit by iroh. Relays can forward encrypted traffic but cannot read voice, messages, attachments, ringtones, or shared-screen data. See [iroh's security and privacy documentation](https://docs.iroh.computer/deployment/security-privacy).

An encrypted connection authenticates the peer's iroh Endpoint ID, not their nickname or real-world identity. Exchange and verify Endpoint IDs through a trusted channel.

Telepathy is not an anonymity service. Direct peers may learn each other's network addresses. Relay, DNS/Pkarr, internet, and local-network operators may observe metadata such as IP addresses, Endpoint ID lookups, timing, duration, and traffic volume.

Content is readable at each endpoint. Recipients, compromised devices, operating systems, browsers, or local malware may save, record, copy, or disclose it.

## Data stored on your device

Profile private keys, Endpoint IDs, nicknames, contacts, and rooms are stored using `flutter_secure_storage`. Ordinary application, audio, and network settings are stored using `shared_preferences`.

Chat history is kept in application memory rather than a persistent chat database. Received attachments are saved to the platform's Downloads or browser download location and remain there until deleted.

A selected custom-ringtone file remains in its original location, and its path is stored in application preferences. The ringtone data may be sent to the called peer during call setup.

Native builds may write diagnostic log files, while web builds may log to the browser console. Logs can contain peer or connection metadata and may contain protocol payloads in some error or debug paths. Review and redact logs before sharing them. Operating-system backups and similar services may also copy local data.

## Permissions and files

Telepathy accesses the microphone when needed for calls or audio testing. Screen sharing and file selection are user-initiated where supported. Treat attachments received from peers as untrusted files; encryption authenticates the sending Endpoint ID but does not make a file safe to open.

## Analytics

Telepathy's application code does not include advertising, remote crash-reporting, or developer-operated usage-analytics SDKs.

The official web app uses [Cloudflare Web Analytics](https://developers.cloudflare.com/web-analytics/) to collect page views, page-load and performance metrics, URL paths, referrers, approximate country, device type, browser, and operating system.

Cloudflare states that this service does not use cookies or `localStorage` for usage metrics and does not fingerprint individual visitors. Cloudflare still receives the network metadata needed to process analytics requests under its own [Privacy Policy](https://www.cloudflare.com/privacypolicy/).

Cloudflare Web Analytics is separate from Telepathy's peer-to-peer connections and is not intended to receive private keys, contacts, messages, attachments, call audio, or screen-sharing content. Native and self-hosted builds do not use the official site's analytics unless their operator adds it.

## Deleting data

Removing a profile deletes its identity, nickname, contacts, and rooms from Telepathy's secure-storage namespace. Removing the final profile creates a new blank profile and identity so the application remains usable.

Profile removal does not delete downloaded attachments, diagnostic logs, the original ringtone file, ordinary preferences, copies retained by peers, backups, or metadata already processed by network and hosting services.

For a broader reset, manually delete downloads and logs, then clear the application's data or browser site storage. Telepathy has no centralized account or server-side message history to delete.

## Security and reporting

End-to-end encryption protects content in transit; it does not prevent endpoint compromise, communication with an incorrectly verified peer, recipient recording, local data exposure, traffic analysis, denial of service, or vulnerabilities in Telepathy and its dependencies.

Report suspected vulnerabilities privately according to [SECURITY.md](SECURITY.md). General privacy questions may be opened as a repository issue or sent to [me@chanchan.dev](mailto:me@chanchan.dev).