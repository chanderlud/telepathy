# Concepts

> Shared domain vocabulary for this project — entities, named processes, and status concepts with project-specific meaning. Seeded with core domain vocabulary, then accretes as ce-compound and ce-compound-refresh process learnings; direct edits are fine. Glossary only, not a spec or catch-all.

## Calling

### Room
A named group call: a saved set of members who can be dialed together. A room's identity is derived from its member set, so the same members always produce the same room regardless of name; renaming never re-keys it. Rooms always include the local user as a member.

### Active Room
The room the local user is currently in a call with. At most one exists at a time. While a room is active, the UI replaces the contacts list with the room's panel in every layout breakpoint, and the room cannot be renamed or deleted.

### Pending Room
A room whose join has been requested but not yet accepted or failed. Like the Active Room, it is lifecycle-locked against rename and delete, because its list row carries the only hangup control while the join is in flight.

### Session
A per-peer network connection, established lazily with each contact. A session has a status (connecting, connected, unavailable, and so on); only a connected session can carry a call.

### Direct vs Relayed
The two flavors of a connected Session. A direct connection runs peer-to-peer; a relayed connection is forwarded through a relay and has higher latency. Connected contact rows label the flavor in text next to the peer's address.

## Identity

### Peer ID
The string-encoded public key that identifies a user on the network. Every Contact and every room member is referenced by peer ID; user input carrying one is validated before use.

### Contact
A saved peer — nickname plus Peer ID — for direct one-to-one calls. Adding a contact also establishes its Session. Contacts double as the nickname source anywhere a peer ID is displayed, including room member lists.

## Sharing

### Room Details
The shareable text encoding of a Room — its name and member peer IDs — that one user pastes to another out-of-band. Pasting room details prefills the add-room form after validating every member's Peer ID.
