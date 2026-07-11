import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/core/utils/console.dart';

/// Shared sound handles used across bootstrap callbacks and UI widgets.
///
/// These are intentionally centralized so the rust callback handlers (wired in
/// `main()`) and the UI (call controls) can coordinate cancelling / replacing
/// sound effects without keeping duplicated logic in `main.dart`.
FlutterSoundHandle? outgoingSoundHandle;
FlutterSoundHandle? otherSoundHandle;

Future<FlutterSoundHandle?> playSoundEffect({
  required SoundPlayer player,
  required List<int> bytes,
  required String sound,
}) async {
  try {
    return await player.play(bytes: bytes);
  } on DartError catch (error) {
    DebugConsole.error('Failed to play $sound sound: ${error.message}');
    return null;
  }
}
