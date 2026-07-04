import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/core/rust/types.dart';

void main() {
  group('StateController.runAudioTest', () {
    test('clears inAudioTest after a successful audio test', () async {
      final controller = StateController();
      final completer = Completer<void>();

      final audioTest = controller.runAudioTest(() => completer.future);

      expect(controller.inAudioTest, isTrue);
      expect(controller.status, 'In Audio Test');

      completer.complete();
      await audioTest;

      expect(controller.inAudioTest, isFalse);
      expect(controller.status, 'Inactive');
    });

    test('clears inAudioTest after a DartError', () async {
      final controller = StateController();
      final completer = Completer<void>();

      final audioTest = controller.runAudioTest(() => completer.future);

      expect(controller.inAudioTest, isTrue);
      expect(controller.status, 'In Audio Test');

      completer.completeError(const DartError(message: 'boom'));

      await expectLater(audioTest, throwsA(isA<DartError>()));

      expect(controller.inAudioTest, isFalse);
      expect(controller.status, 'Inactive');
    });
  });
}
