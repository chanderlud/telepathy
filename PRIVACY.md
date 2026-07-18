# Privacy and Security

Telepathy is a peer-to-peer voice, text, attachment, and screen-sharing application built with Flutter, Rust, and [iroh](https://docs.iroh.computer/). This document explains what Telepathy stores, what it sends over the network, what third parties can observe, and what Telepathy's use of “secure” does and does not mean.

It covers the open-source application and the official web deployment at [telepathy.chanchan.dev](https://telepathy.chanchan.dev/). Other deployments, app stores, operating systems, relays, DNS resolvers, and distribution services may process additional data under their own terms.

## At a glance

- **Communication content is end-to-end encrypted in transit by iroh.** A relay carrying a connection cannot decrypt the voice, text, attachments, ringtone, or screen-sharing data sent over that connection.
- **Telepathy authenticates an iroh Endpoint ID, not a real-world person or nickname.** Users must exchange and verify Endpoint IDs through a trusted channel.
- **Telepathy is not an anonymity system.** Direct peers exchange network addresses, and a relay can observe connection metadata even though it cannot read content.
- **The application code does not include advertising or remote crash-reporting SDKs.** The official web app uses Cloudflare Web Analytics to collect website traffic, page-load, and performance metrics.
- **Profile identity data is stored through `flutter_secure_storage`.** Ordinary application, audio, and network settings are stored through `shared_preferences`.
- **Chat history is held in memory rather than a persistent chat database.** Received attachments, diagnostic logs, and the user's original custom-ringtone file can remain on disk.
- **Diagnostic logs are not guaranteed to be content-free.** They can contain peer and connection metadata and, in some debug or exceptional code paths, may contain protocol payloads. Review and redact logs before sharing them.

## Data Telepathy handles

### Local profile and identity

Each Telepathy profile has an iroh cryptographic identity. Telepathy stores the following profile data locally:

- the profile's private iroh key;
- the corresponding public Endpoint ID;
- the profile nickname;
- contacts, including locally assigned nicknames, public Endpoint IDs, and per-contact output volume;
- room definitions and room-member Endpoint IDs; and
- internal profile identifiers and the active profile.

The private key is the credential for the Endpoint ID. Anyone who obtains that key can impersonate the profile. Telepathy does not provide a centralized account, password reset, key escrow, or identity-recovery service.

Nicknames are local labels. A nickname is not cryptographically authenticated and is not proof of a person's identity.

### Communications

Depending on the features used, Telepathy processes:

- microphone audio;
- text messages;
- attachment names and file contents;
- call state and control messages;
- a selected custom-ringtone file;
- screen-sharing video frames; and
- room membership and participant information.

This content is available in plaintext at the sending and receiving endpoints because the application must capture, display, play, or save it there. End-to-end encryption protects content while it travels between endpoints; it does not prevent an intended recipient, compromised device, operating system, or local malware from reading or recording it.

### Microphone and screen capture

On web and mobile builds, Telepathy requests microphone permission during startup. Microphone audio is captured and processed when Telepathy creates an input stream for a call or audio test. Telepathy does not create a call-recording archive, but a peer or the operating system can still record a call outside Telepathy.

Screen sharing is user-initiated and is available only where the feature is supported. Shared frames are sent to the selected peer or peers. Telepathy does not add technical restrictions that prevent recipients from recording or redistributing a shared screen.

### Custom ringtones

On supported native builds, a configured custom-ringtone file can be read from disk and sent as bytes to the called peer as part of call setup. This can happen before the peer accepts the call so that the ringtone can be played while the call is ringing.

Do not select a private or sensitive audio file as a custom ringtone. The called peer receives a copy, and the ringtone's local source path is stored in ordinary application preferences on the caller's device.

### Attachments

Outgoing attachments are read into memory and sent to the selected peer over the encrypted iroh connection.

Received attachments are saved automatically:

- on most native platforms, under a `Telepathy` folder in the user's Downloads location when that location is available;
- on iOS, in an application-accessible/sandboxed downloads location; and
- on the web, through the browser's normal download mechanism.

After a received attachment is saved, Telepathy drops its attachment bytes from the in-memory message object. Image previews may remain temporarily in an in-memory cache. Saved files are not automatically deleted when chat state or a profile is cleared.

Treat an attachment from another peer as untrusted input. End-to-end encryption confirms which Endpoint ID sent data over the authenticated connection; it does not make the file safe to open.

## Local storage and retention

| Data | Storage | Typical retention |
| --- | --- | --- |
| Profile private key | `flutter_secure_storage` using platform-specific secure storage | Until the profile is deleted, application/site storage is cleared, or platform storage removes it |
| Endpoint ID, profile nickname, contacts, and rooms | `flutter_secure_storage` | Until changed or the profile is deleted |
| Profile list and active-profile identifier | `shared_preferences` | Until changed, app/site data is cleared, or the app is uninstalled, subject to platform behavior |
| Audio, UI, and network settings | `shared_preferences` | Until changed, reset, or app/site data is cleared |
| Custom-ringtone source path | `shared_preferences`; the selected file remains in its original location | Until changed/reset; the source file is controlled by the user and operating system |
| Chat messages | Application memory; Telepathy does not use a persistent chat-history database | Until chat state is cleared or the process ends; operating-system process lifetime varies |
| Outgoing attachment/ringtone bytes | Application memory during selection, encoding, and transfer | Normally released after use, subject to normal process-memory behavior |
| Received attachments | Downloads/app files or browser download location | Until manually deleted by the user or operating system |
| In-app diagnostic console | Application memory | Until cleared or the process ends |
| Native Rust diagnostic logs | Daily JSON trace files with the `telepathy-trace.log` prefix in the process working directory | Telepathy does not automatically prune them; files remain until manually removed |
| Web diagnostics | Browser developer console | Controlled by the browser and developer tools |

`flutter_secure_storage` uses platform-specific secure-storage mechanisms. Its exact protection, accessibility, backup, and deletion behavior depends on the operating system, browser, device configuration, and plugin version. Telepathy constructs the plugin with its default options and does not require biometric authentication.

`shared_preferences` is ordinary platform preference storage, not Telepathy's secure secret-storage layer. It is used for non-secret configuration such as relay and DNS URLs, bind/listen settings, audio device identifiers, volume and denoise settings, display preferences, and the custom-ringtone path.

Operating-system backup, roaming-profile, browser-sync, filesystem, or device-management features may copy local application data. Telepathy does not control those platform features.

## Diagnostic logging

Telepathy uses structured diagnostic logging for troubleshooting.

On native builds, the Rust layer writes daily JSON trace files in the process working directory and also forwards logs to an in-app debug console. Web builds can write diagnostics to the browser console. Release builds use a less verbose default filter than debug builds, but warning and error events can still be recorded.

Logs can include information such as:

- public Endpoint IDs and locally assigned nicknames;
- session and connection identifiers;
- whether a connection was relayed;
- socket/IP addresses and connection timing;
- room identifiers or hashes;
- selected audio-device identifiers;
- configuration values, status changes, and error details; and
- in some debug or unexpected/error paths, a debug representation of a protocol message, which may include text or binary payload data.

Diagnostic logs are local application output, not product telemetry, but they can still contain sensitive information. Do not publish or attach a log without inspecting and redacting it. Native logs are not deleted when a profile is removed.

## Network services and metadata

### No centralized Telepathy message service

Telepathy does not use a Telepathy-operated account or message-storage server. Peers connect using iroh. This does not mean that no third party receives network metadata: iroh relay and address-lookup services, DNS resolvers, internet providers, hosting providers, operating systems, and app stores can receive the operational data normally visible to them.

Starting Telepathy can cause its iroh endpoint to contact relay and address-lookup infrastructure even before communication content is sent.

### Direct connections

Iroh attempts to establish a direct connection when possible. To perform NAT traversal, peers exchange candidate network addresses, including public and potentially local/private addresses, through encrypted coordination messages.

A direct peer can therefore learn network addresses associated with the other endpoint. Those addresses may reveal an approximate location, network provider, workplace or home network, or other identifying context. Telepathy is not designed to hide participants' IP addresses from each other.

### Relay connections

When a direct path is unavailable or not yet established, iroh can route encrypted traffic through a relay. Telepathy uses iroh's default public relay configuration unless the user supplies custom network settings.

According to iroh's security documentation, a relay cannot decrypt application content, but it can observe metadata such as:

- source and destination IP addresses;
- when connections occur and how long they last; and
- the amount and timing pattern of data relayed.

Iroh describes its relays as stateless and says they do not store application data, but an operator may process operational or abuse-prevention metadata. The public relay service is operated by n0 and is governed by that operator's policies. A custom relay operator can observe comparable connection metadata and may have different retention or monitoring practices.

Iroh's own documentation advises users not to rely on its public relays for sensitive or confidential use cases where this metadata exposure or public-service operation is unacceptable. Self-hosting a relay changes who operates the relay; it does not make traffic metadata invisible to that operator.

### Endpoint address lookup

An iroh Endpoint ID is the public half of an Ed25519 keypair. To connect, a peer also needs current addressing information.

By default, Telepathy configures iroh's n0 DNS/Pkarr address-lookup services. The default publisher creates a signed record that maps the Endpoint ID to its home relay URL. Iroh's default publisher does not publish direct IP addresses in that record, although direct addresses are still exchanged between peers during connection establishment.

Address lookup has privacy consequences:

- the Endpoint ID appears in the lookup name or request;
- the address-lookup service and DNS resolver can observe the requesting network address, lookup timing, and which Endpoint ID is being resolved; and
- the published record exposes the endpoint's home-relay URL.

The record is signed so that peers can verify that its addressing information was authorized by the Endpoint ID's private key. Signing protects authenticity; it does not hide the record or the lookup metadata.

Telepathy supports custom relay, DNS, and Pkarr endpoints through its network settings. The privacy and retention practices of a custom service are the responsibility of its operator.

## Encryption and peer authentication

Telepathy relies on iroh's transport security rather than adding a second, independent message-encryption layer.

Iroh documents the following properties for endpoint connections:

- traffic between endpoints is end-to-end encrypted;
- connections use QUIC with TLS 1.3;
- the channel is authenticated to the remote Endpoint ID;
- relays forward ciphertext and cannot read application content; and
- established channels provide forward and backward secrecy under iroh's stated security assumptions.

For Telepathy, that transport carries voice packets, text, attachments, call-control messages, custom ringtones, and screen-sharing data. In a room or multi-peer session, privacy is evaluated per connection: every intended participant is an endpoint that can decrypt the content delivered to that participant.

The cryptographic identity is the Endpoint ID. A successful encrypted connection proves that the remote endpoint controls the private key corresponding to that Endpoint ID. It does **not** prove that:

- the Endpoint ID belongs to the person whose nickname is displayed;
- the person is using an uncompromised device;
- the recipient will keep content confidential; or
- an Endpoint ID obtained through an untrusted channel was not substituted by an attacker.

Exchange and verify Endpoint IDs through a trusted, authenticated channel. Telepathy does not implement a certificate-authority identity, username registry, safety-number comparison ceremony, or other independent real-world identity verification.

The exact cryptographic algorithms available and negotiated can vary with iroh, the target platform, and the build configuration. Telepathy relies on iroh's documented endpoint-level guarantees rather than promising one fixed cipher suite for every build.

## What each party can see

| Party | Can read communication content? | Other information it can observe |
| --- | --- | --- |
| Intended peer/room participant | Yes, for content delivered to that endpoint | Endpoint ID, call/message timing, attachment names, and network addresses used or exchanged during connectivity checks |
| Iroh relay operator | No, under iroh's documented end-to-end encryption model | Source/destination network addresses, timing, duration, and traffic volume/patterns; operational and abuse-prevention data |
| DNS/Pkarr/address-lookup service | No communication content | Endpoint ID lookups, requester network metadata, timing, and the signed home-relay mapping |
| Internet provider or local network operator | Normally not content protected by iroh | IP-level destinations, timing, volume, and whether known relay/address-lookup infrastructure is contacted |
| Telepathy on the local device | Yes, as needed to capture, display, play, and save content | Profile data, local preferences, in-memory messages, selected files, and diagnostic events |
| Operating system/browser/local malware | Potentially, depending on device security | Application memory, permissions, secure-storage access under platform rules, preferences, downloads, logs, clipboard, screen, and audio devices |
| Cloudflare Web Analytics, for the official hosted web app | No; it is separate from the iroh peer-to-peer transport | Visits, page views, page-load and performance metrics, URL paths, referrers, approximate country, device type, browser, operating system, and network metadata needed to receive analytics requests |
| Other web hosts or software distribution services | Not through the Telepathy protocol itself | Normal website/app-store/CDN request metadata and any data described by that service's own policy |

## Telemetry and tracking

### Application telemetry

Telepathy's Flutter and Rust application code does not include advertising or remote crash-reporting SDKs, and it does not send feature-use events or communication content to the developer. Telepathy still sends data to peers and contacts network infrastructure for connectivity, as described above. The official hosted web app has separate website analytics, described below.

### Official hosted web app

The official web build at [telepathy.chanchan.dev](https://telepathy.chanchan.dev/) uses [Cloudflare Web Analytics](https://developers.cloudflare.com/web-analytics/) to understand visits and web-app performance. Cloudflare collects metrics such as visits, page views, page-load timing, Core Web Vitals, URL paths, referrers, approximate country, device type, browser, and operating system.

Cloudflare states that Web Analytics does not use cookies or `localStorage` to collect usage metrics and does not fingerprint individuals using IP addresses, user-agent strings, or other data. Cloudflare still receives the network metadata needed to receive and process analytics requests, and handles that information under its own [Privacy Policy](https://www.cloudflare.com/privacypolicy/).

Cloudflare Web Analytics is separate from Telepathy's iroh peer-to-peer transport. It is used to measure loading and performance of the hosted web app and is not intended to receive private keys, contact lists, messages, attachments, call audio, or screen-sharing content.

Other hosted deployments may use different hosting, logging, or analytics services. Native and self-hosted builds do not use the official site's Cloudflare Web Analytics configuration unless their distributor or operator adds it.

## Deleting local data

To remove a Telepathy profile, use the application's profile-removal function. Telepathy removes that profile's private key, Endpoint ID, nickname, contacts, and rooms from its secure-storage namespace. When the last profile is removed, Telepathy creates a new blank default profile with a new identity so that the application remains usable.

Profile removal does **not** automatically delete:

- received files in Downloads or the browser's download location;
- native `telepathy-trace.log` files;
- the original file selected as a custom ringtone;
- ordinary audio, network, and UI preferences;
- copies retained by peers;
- metadata already observed or retained by relay, DNS, hosting, operating-system, or distribution services; or
- device, filesystem, cloud, or browser backups.

For a broader local reset, remove profiles, manually delete downloaded attachments and diagnostic logs, and clear the application's data or browser site storage using the operating system/browser. Uninstall behavior and backup retention vary by platform. There is no centralized Telepathy account from which to request global deletion or revocation of an Endpoint ID.

## Security and privacy limitations

Telepathy's end-to-end encrypted transport is intended to protect content from relays and passive network observers between the communicating endpoints. It does not protect against every threat. In particular, it does not prevent:

- a malicious or careless recipient from saving, forwarding, recording, or screenshotting content;
- compromise of an endpoint, private key, operating system, browser, or audio/video device;
- communication with the wrong person after an Endpoint ID is exchanged through an untrusted channel;
- disclosure through local downloads, preferences, process memory, or diagnostic logs;
- IP-address, timing, traffic-volume, relay, and address-lookup metadata exposure;
- denial of service, blocking, traffic analysis, or service availability failures; or
- vulnerabilities in Telepathy, iroh, Flutter, Rust dependencies, or the underlying platform.

Telepathy is not an anonymity network, and this document does not claim that the application has undergone an independent security audit.

## Reporting privacy or security issues

Do not include private keys, message contents, attachments, unredacted logs, or exploit details in a public issue.

The repository does not publish a dedicated `SECURITY.md` or private vulnerability-reporting procedure. Until one is added, email [me@chanchan.dev](mailto:me@chanchan.dev) with the subject `Telepathy security report` for sensitive reports. General privacy questions that contain no sensitive information may be opened as a repository issue.

## References

The behavior described here is implemented in Telepathy's Flutter and Rust source. Relevant source and upstream documentation include:

- [Telepathy source repository](https://github.com/chanderlud/telepathy)
- [Telepathy Flutter source](https://github.com/chanderlud/telepathy/tree/master/lib)
- [Telepathy Rust source](https://github.com/chanderlud/telepathy/tree/master/rust)
- [iroh: Security & Privacy](https://docs.iroh.computer/deployment/security-privacy)
- [iroh: FAQ](https://docs.iroh.computer/about/faq)
- [iroh: Endpoints](https://docs.iroh.computer/concepts/endpoints)
- [iroh: DNS address lookup](https://docs.iroh.computer/connecting/dns-address-lookup)
- [`flutter_secure_storage` documentation](https://pub.dev/packages/flutter_secure_storage)
- [`shared_preferences` documentation](https://pub.dev/packages/shared_preferences)
- [Cloudflare Web Analytics](https://developers.cloudflare.com/web-analytics/)
- [Cloudflare Web Analytics: high-level metrics](https://developers.cloudflare.com/web-analytics/data-metrics/high-level-metrics/)
- [Cloudflare Web Analytics: data origin and collection](https://developers.cloudflare.com/web-analytics/data-metrics/data-origin-and-collection/)
- [Cloudflare Privacy Policy](https://www.cloudflare.com/privacypolicy/)

Keep this file aligned with Telepathy's networking defaults, storage, logging, permissions, telemetry, hosted services, and content-retention behavior.
