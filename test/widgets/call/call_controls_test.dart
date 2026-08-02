import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/audio_settings_controller.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/widgets/call/call_controls.dart';

import '../../support/fake_contact.dart';

class _OpaqueTelepathy implements Telepathy {
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _OpaquePlayer implements SoundPlayer {
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

VideoSessionIdentity _identity() => VideoSessionIdentity(
      peerId: 'desktop-peer',
      sessionId: VideoSessionId(
        field0: U8Array16(Uint8List.fromList(List<int>.filled(16, 1))),
      ),
    );

VideoCapabilities _capabilities({required bool available}) => VideoCapabilities(
      send: available
          ? const VideoCapabilityAvailability.available()
          : const VideoCapabilityAvailability.unavailable(
              VideoUnavailable.runtimeUnavailable(),
            ),
      receive: const VideoCapabilityAvailability.available(),
      sendSources: available
          ? const [
              VideoSourceCapability(source: VideoSource.display, formats: [])
            ]
          : const [],
      receiveFormats: const [],
    );

Future<void> _pumpControls(
  WidgetTester tester, {
  required StateController state,
  required VideoControlActions actions,
}) async {
  final audio = AudioSettingsController(options: SharedPreferencesAsync());
  await audio.init();
  addTearDown(audio.dispose);
  await tester.pumpWidget(
    MultiProvider(
      providers: [
        ChangeNotifierProvider.value(value: state),
        ChangeNotifierProvider.value(value: audio),
        Provider<Telepathy>.value(value: _OpaqueTelepathy()),
        Provider<SoundPlayer>.value(value: _OpaquePlayer()),
      ],
      child: MaterialApp(
          home: Scaffold(body: CallControls(videoActions: actions))),
    ),
  );
}

void main() {
  setUp(() {
    SharedPreferencesAsyncPlatform.instance =
        InMemorySharedPreferencesAsync.empty();
  });

  tearDown(() {
    SharedPreferencesAsyncPlatform.instance = null;
  });

  testWidgets('desktop control requests display and locally stops its identity',
      (tester) async {
    final state = StateController();
    final contact = FakeContact(id: 'desktop-peer', contactNickname: 'Desktop');
    final identity = _identity();
    var requestCalls = 0;
    final stopped = <VideoSessionIdentity>[];
    final actions = VideoControlActions(
      videoCapabilities: () async => _capabilities(available: true),
      isSourceConfigured: (_) async => true,
      requestDisplay: (_) async {
        requestCalls += 1;
        return VideoStartOutcome.requested(identity);
      },
      stop: (value) async {
        stopped.add(value);
        return VideoStopOutcome.stopped;
      },
    );
    state.promotePendingCallAttempt(state.setPendingContact(contact));

    await _pumpControls(tester, state: state, actions: actions);
    expect(find.bySemanticsLabel('Screenshare icon'), findsOneWidget);
    final screenshareButton = find.ancestor(
      of: find.bySemanticsLabel('Screenshare icon'),
      matching: find.byType(IconButton),
    );
    tester.widget<IconButton>(screenshareButton).onPressed!();
    await tester.pump();

    expect(requestCalls, 1);
    expect(state.isSendingScreenshare, isFalse,
        reason: 'request acceptance must not create pending UI');

    state.handleVideoLifecycle(VideoLifecycleEvent(
      identity: identity,
      role: VideoRole.sender,
      source: VideoSource.display,
      phase: VideoPhase.active,
    ));
    await tester.pump();
    tester.widget<IconButton>(screenshareButton).onPressed!();
    await tester.pump();

    expect(stopped, [identity]);
    expect(state.isSendingScreenshare, isFalse,
        reason: 'local stop clears the existing sending state immediately');
  });

  testWidgets('unavailable generic start preserves the existing message',
      (tester) async {
    final state = StateController();
    state.promotePendingCallAttempt(state.setPendingContact(
      FakeContact(id: 'desktop-peer', contactNickname: 'Desktop'),
    ));
    final actions = VideoControlActions(
      videoCapabilities: () async => _capabilities(available: true),
      isSourceConfigured: (_) async => true,
      requestDisplay: (_) async => const VideoStartOutcome.unavailable(
        VideoUnavailable.runtimeUnavailable(),
      ),
      stop: (_) async => VideoStopOutcome.stopped,
    );

    await _pumpControls(tester, state: state, actions: actions);
    final screenshareButton = find.ancestor(
      of: find.bySemanticsLabel('Screenshare icon'),
      matching: find.byType(IconButton),
    );
    tester.widget<IconButton>(screenshareButton).onPressed!();
    await tester.pumpAndSettle();

    expect(find.text('Screenshare Unavailable'), findsOneWidget);
    expect(state.isSendingScreenshare, isFalse);
  });
}
