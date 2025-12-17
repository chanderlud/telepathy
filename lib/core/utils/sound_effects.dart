import 'package:telepathy/src/rust/audio/player.dart';

/// Shared sound handles used across bootstrap callbacks and UI widgets.
///
/// These are intentionally centralized so the rust callback handlers (wired in
/// `main()`) and the UI (call controls) can coordinate cancelling / replacing
/// sound effects without keeping duplicated logic in `main.dart`.
SoundHandle? outgoingSoundHandle;
SoundHandle? otherSoundHandle;
