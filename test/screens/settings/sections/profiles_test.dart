import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/screens/settings/sections/profiles.dart';
import 'package:telepathy/widgets/common/index.dart';

void main() {
  setUp(() {
    SharedPreferencesAsyncPlatform.instance =
        InMemorySharedPreferencesAsync.empty();
  });

  tearDown(() {
    SharedPreferencesAsyncPlatform.instance = null;
  });

  testWidgets('pressing Enter in create dialog creates a profile', (
    WidgetTester tester,
  ) async {
    final profilesController = FakeProfilesController();

    await tester.pumpProfileSettings(profilesController);
    await tester.openCreateProfileDialog();

    await tester.enterText(find.byType(TextField), 'Keyboard Profile');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();

    expect(find.text('Create Profile'), findsNothing);
    expect(find.text('Keyboard Profile'), findsOneWidget);
    expect(profilesController.createdNames, <String>['Keyboard Profile']);

    await tester.openCreateProfileDialog();
    final textField = tester.widget<TextField>(find.byType(TextField));
    expect(textField.controller?.text, isEmpty);
  });

  testWidgets('Create button still creates a profile', (
    WidgetTester tester,
  ) async {
    final profilesController = FakeProfilesController();

    await tester.pumpProfileSettings(profilesController);
    await tester.openCreateProfileDialog();

    await tester.enterText(find.byType(TextField), 'Button Profile');
    await tester.tap(find.widgetWithText(ElevatedButton, 'Create'));
    await tester.pumpAndSettle();

    expect(find.text('Create Profile'), findsNothing);
    expect(find.text('Button Profile'), findsOneWidget);
    expect(profilesController.createdNames, <String>['Button Profile']);
  });

  testWidgets('Create button rejects empty profile names', (
    WidgetTester tester,
  ) async {
    final profilesController = FakeProfilesController();

    await tester.pumpProfileSettings(profilesController);
    await tester.openCreateProfileDialog();

    await tester.enterText(find.byType(TextField), '   ');
    await tester.tap(find.widgetWithText(ElevatedButton, 'Create'));
    await tester.pumpAndSettle();

    expect(find.text('Create Profile'), findsOneWidget);
    expect(find.text('Profile name is required.'), findsOneWidget);
    expect(profilesController.createdNames, isEmpty);
  });

  testWidgets('create dialog rejects duplicate profile names', (
    WidgetTester tester,
  ) async {
    final profilesController = FakeProfilesController();

    await profilesController.createProfile('Existing Profile');
    profilesController.createdNames.clear();

    await tester.pumpProfileSettings(profilesController);
    await tester.openCreateProfileDialog();

    await tester.enterText(find.byType(TextField), '  existing profile  ');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();

    expect(find.text('Create Profile'), findsOneWidget);
    expect(
      find.text('A profile named "existing profile" already exists.'),
      findsOneWidget,
    );
    expect(profilesController.createdNames, isEmpty);
  });

  group('ProfileSettings "Set Active" gate', () {
    // Regression: the previous gate was `stateController.isCallActive`, which
    // is only true once `setActiveContact`/`setActiveRoom` has promoted the slot.
    // During `CallLifecycle.connecting` (and during audio tests) the backend call
    // slot is already occupied but `isCallActive` is still false, so the button
    // stayed enabled and tapping `restartManager()` could race the in-flight
    // startCall and leave the frontend profile and backend identity inconsistent.
    // The gate is now `blockAudioChanges`, covering connecting + active + audio-test.

    testWidgets(
        'the "Set Active" button is disabled while a pending room is in '
        'the connecting phase', (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('First Profile');
      final stateController = StateController();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
      );

      stateController.setPendingRoom(_roomFixture('connect-pending'));
      await tester.pumpAndSettle();

      final setActiveButton = find.widgetWithText(Button, 'Set Active');
      expect(setActiveButton, findsOneWidget);
      expect(tester.widget<Button>(setActiveButton).disabled, isTrue,
          reason: 'Set Active must be disabled while a call slot is pending '
              'in the connecting phase; isCallActive alone misses this');

      await tester.tap(setActiveButton);
      await tester.pumpAndSettle();

      expect(profilesController.setActiveCalls, isEmpty,
          reason: 'the defensive early return inside onPressed must reject '
              'mutations while blockAudioChanges is true');
    });

    testWidgets('the "Set Active" button is disabled during an audio test',
        (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Audio Gate Profile');
      final stateController = StateController();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
      );

      // setInAudioTest flips `inAudioTest` and `blockAudioChanges` without
      // requiring the rust audio-test bridge.
      stateController.setInAudioTest(true);
      await tester.pumpAndSettle();

      final setActiveButton = find.widgetWithText(Button, 'Set Active');
      expect(setActiveButton, findsOneWidget);
      expect(tester.widget<Button>(setActiveButton).disabled, isTrue,
          reason: 'Set Active must be disabled during audio tests because '
              'restartManager() competes with the audio-test device '
              'access the backend holds');
    });

    testWidgets('the "Set Active" button stays disabled during call teardown',
        (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Teardown Gate Profile');
      final stateController = StateController();
      final room = _roomFixture('teardown-pending');
      final attempt = stateController.setPendingRoom(room);
      stateController.promotePendingCallAttempt(attempt);
      stateController.beginCallEnding();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
      );

      final setActiveButton = find.widgetWithText(Button, 'Set Active');
      expect(tester.widget<Button>(setActiveButton).disabled, isTrue,
          reason: 'profile changes must not race backend slot teardown');

      await tester.tap(setActiveButton);
      await tester.pumpAndSettle();
      expect(profilesController.setActiveCalls, isEmpty);
    });

    testWidgets(
        'when the controller is idle, "Set Active" runs the full '
        'setActiveProfile / setIdentity / restartManager sequence',
        (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Idle Profile');
      final telepathy = _RecordingTelepathy();
      final stateController = StateController();

      await tester.pumpProfileSettingsWithAll(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      expect(
        tester
            .widget<Button>(find.widgetWithText(Button, 'Set Active'))
            .disabled,
        isFalse,
        reason: 'idle controller must leave the button enabled',
      );

      await tester.tap(find.widgetWithText(Button, 'Set Active'));
      await tester.pumpAndSettle();

      expect(profilesController.setActiveCalls, ['profile-0'],
          reason: 'profile switch must reach profilesController');
      expect(telepathy.identityCalls, hasLength(1),
          reason: 'profile switch must push the keypair through setIdentity');
      expect(telepathy.restartManagerCalls, hasLength(1),
          reason: 'profile switch must restart the manager after the identity '
              'has been applied');
    });
  });
}

Room _roomFixture(String id) => Room(
      id: id,
      peerIds: <String>[],
      nickname: 'Room $id',
    );

extension on WidgetTester {
  Future<void> pumpProfileSettings(FakeProfilesController profilesController) {
    return pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<ProfilesController>.value(
            value: profilesController,
          ),
          ChangeNotifierProvider<StateController>.value(
            value: StateController(),
          ),
          Provider<Telepathy>.value(value: FakeTelepathy()),
        ],
        child: const MaterialApp(home: Scaffold(body: ProfileSettings())),
      ),
    );
  }

  /// Variant used by the "Set Active" gate tests: lets the test supply the
  /// `StateController` (or `Telepathy`) so the controller can be advanced into
  /// `connecting` / `audio-test` before the widget mounts.
  Future<void> pumpProfileSettingsWithState({
    required FakeProfilesController profilesController,
    required StateController stateController,
    Telepathy? telepathy,
  }) {
    return pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<ProfilesController>.value(
            value: profilesController,
          ),
          ChangeNotifierProvider<StateController>.value(
            value: stateController,
          ),
          Provider<Telepathy>.value(value: telepathy ?? FakeTelepathy()),
        ],
        child: const MaterialApp(home: Scaffold(body: ProfileSettings())),
      ),
    );
  }

  Future<void> pumpProfileSettingsWithAll({
    required FakeProfilesController profilesController,
    required StateController stateController,
    required Telepathy telepathy,
  }) {
    return pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<ProfilesController>.value(
            value: profilesController,
          ),
          ChangeNotifierProvider<StateController>.value(
            value: stateController,
          ),
          Provider<Telepathy>.value(value: telepathy),
        ],
        child: const MaterialApp(home: Scaffold(body: ProfileSettings())),
      ),
    );
  }

  Future<void> openCreateProfileDialog() async {
    await tap(find.byTooltip('Create Profile'));
    await pumpAndSettle();
    expect(find.text('Create Profile'), findsOneWidget);
    expect(find.byType(TextField), findsOneWidget);
  }
}

class FakeProfilesController extends ProfilesController {
  FakeProfilesController()
      : super(
          storage: const FlutterSecureStorage(),
          options: SharedPreferencesAsync(),
        );

  final List<String> createdNames = <String>[];
  final List<String> setActiveCalls = <String>[];
  int _nextProfileId = 0;

  @override
  Future<String> createProfile(String nickname) async {
    final id = 'profile-${_nextProfileId++}';
    createdNames.add(nickname);
    profiles[id] = Profile(
      id: id,
      nickname: nickname.trim().isEmpty ? 'Unnamed Profile' : nickname,
      peerId: 'peer-$id',
      keypair: const <int>[],
      contacts: <String, Contact>{},
      rooms: <String, Room>{},
    );
    notifyListeners();
    return id;
  }

  @override
  Future<void> setActiveProfile(String profileId) async {
    setActiveCalls.add(profileId);
    await super.setActiveProfile(profileId);
  }
}

/// Records `setIdentity` and `restartManager` so the "idle" gate test can
/// verify the full profile-switch sequence fires only when the controller is not
/// blocking audio changes.
class _RecordingTelepathy implements Telepathy {
  final List<List<int>> identityCalls = <List<int>>[];
  final List<void> restartManagerCalls = <void>[];

  @override
  Future<void> setIdentity({required List<int> key}) async {
    identityCalls.add(List<int>.unmodifiable(key));
  }

  @override
  Future<void> restartManager() async {
    restartManagerCalls.add(null);
  }

  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  Future<void> audioTest() async {}

  @override
  ChatMessage buildChat({
    required Contact contact,
    required String text,
    required List<(String, Uint8List)> attachments,
  }) {
    throw UnimplementedError();
  }

  @override
  Future<void> endCall() async {}

  @override
  Future<void> joinRoom(
      {required List<String> memberStrings,
      required StartOperation operation}) async {}

  @override
  Future<(List<AudioDevice>, List<AudioDevice>)> listDevices() async =>
      (<AudioDevice>[], <AudioDevice>[]);

  @override
  StartOperation newStartOperation() => throw UnimplementedError();

  @override
  void pauseStatistics() {}

  @override
  void resumeStatistics() {}

  @override
  Future<void> sendChat({required ChatMessage message}) async {}

  @override
  void setContactOutputVolume({required Contact contact}) {}

  @override
  void setDeafened({required bool deafened}) {}

  @override
  void setDenoise({required bool denoise}) {}

  @override
  void setEfficiencyMode({required bool enabled}) {}

  @override
  Future<void> setInputDevice({String? deviceId}) async {}

  @override
  void setInputVolume({required double decibel}) {}

  @override
  Future<void> setModel({Uint8List? model}) async {}

  @override
  void setMuted({required bool muted}) {}

  @override
  Future<void> setOutputDevice({String? deviceId}) async {}

  @override
  void setOutputVolume({required double decibel}) {}

  @override
  void setPlayCustomRingtones({required bool play}) {}

  @override
  void setRmsThreshold({required double decimal}) {}

  @override
  void setSendCustomRingtone({required bool send}) {}

  @override
  Future<void> shutdown() async {}

  @override
  Future<void> startCall(
      {required Contact contact, required StartOperation operation}) async {}

  @override
  Future<void> startManager() async {}

  @override
  Future<void> startScreenshare({required Contact contact}) async {}

  @override
  Future<void> startSession({required Contact contact}) async {}

  @override
  Future<void> stopSession({required Contact contact}) async {}
}

class FakeTelepathy implements Telepathy {
  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  Future<void> audioTest() async {}

  @override
  ChatMessage buildChat({
    required Contact contact,
    required String text,
    required List<(String, Uint8List)> attachments,
  }) {
    throw UnimplementedError();
  }

  @override
  Future<void> endCall() async {}

  @override
  Future<void> joinRoom(
      {required List<String> memberStrings,
      required StartOperation operation}) async {}

  @override
  Future<(List<AudioDevice>, List<AudioDevice>)> listDevices() async =>
      (<AudioDevice>[], <AudioDevice>[]);

  @override
  StartOperation newStartOperation() => throw UnimplementedError();

  @override
  void pauseStatistics() {}

  @override
  Future<void> restartManager() async {}

  @override
  void resumeStatistics() {}

  @override
  Future<void> sendChat({required ChatMessage message}) async {}

  @override
  void setContactOutputVolume({required Contact contact}) {}

  @override
  void setDeafened({required bool deafened}) {}

  @override
  void setDenoise({required bool denoise}) {}

  @override
  void setEfficiencyMode({required bool enabled}) {}

  @override
  Future<void> setIdentity({required List<int> key}) async {}

  @override
  Future<void> setInputDevice({String? deviceId}) async {}

  @override
  void setInputVolume({required double decibel}) {}

  @override
  Future<void> setModel({Uint8List? model}) async {}

  @override
  void setMuted({required bool muted}) {}

  @override
  Future<void> setOutputDevice({String? deviceId}) async {}

  @override
  void setOutputVolume({required double decibel}) {}

  @override
  void setPlayCustomRingtones({required bool play}) {}

  @override
  void setRmsThreshold({required double decimal}) {}

  @override
  void setSendCustomRingtone({required bool send}) {}

  @override
  Future<void> shutdown() async {}

  @override
  Future<void> startCall(
      {required Contact contact, required StartOperation operation}) async {}

  @override
  Future<void> startManager() async {}

  @override
  Future<void> startScreenshare({required Contact contact}) async {}

  @override
  Future<void> startSession({required Contact contact}) async {}

  @override
  Future<void> stopSession({required Contact contact}) async {}
}
