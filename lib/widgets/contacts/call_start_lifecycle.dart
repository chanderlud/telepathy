import 'package:flutter/material.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/core/utils/index.dart';

Future<void> runOutgoingCallStartLifecycle({
  required BuildContext context,
  required StateController stateController,
  required SoundPlayer player,
  required int? attempt,
  required Future<void> Function() startRequest,
  VoidCallback? onStartAccepted,
}) async {
  try {
    await startRequest();
    if (!stateController.isCurrentCallAttempt(attempt) ||
        stateController.callLifecycle != CallLifecycle.connecting) {
      return;
    }
    onStartAccepted?.call();
    // `CallState.connected` owns promotion. This continuation only confirms
    // backend request acceptance.
    final List<int> bytes = await readSeaBytes('outgoing');
    if (!stateController.isCurrentCallAttempt(attempt) ||
        stateController.callLifecycle != CallLifecycle.connecting) {
      return;
    }
    final FlutterSoundHandle? soundHandle = await playSoundEffect(
      player: player,
      bytes: bytes,
      sound: 'outgoing',
    );
    if (!stateController.isCurrentCallAttempt(attempt) ||
        stateController.callLifecycle != CallLifecycle.connecting) {
      soundHandle?.cancel();
      return;
    }
    outgoingSoundHandle = soundHandle;
  } on DartError catch (error) {
    if (!stateController.isCurrentCallAttempt(attempt) ||
        stateController.callLifecycle != CallLifecycle.connecting) {
      return;
    }
    stateController.endOfCall();
    outgoingSoundHandle?.cancel();
    if (!context.mounted) return;
    showErrorDialog(context, 'Call failed', error.message);
  } finally {
    stateController.settleStartAttempt(attempt);
  }
}
