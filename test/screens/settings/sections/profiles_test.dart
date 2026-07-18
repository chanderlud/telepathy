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
        'when the controller is idle, "Set Active" runs the atomic '
        'switchIdentityAndRestartManager and commits the frontend swap '
        'only after the backend accepts it', (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Idle Profile A');
      await profilesController.createProfile('Idle Profile B');
      // The fake does not auto-select the first profile; pick the second so
      // tapping the first row's "Set Active" actually changes the active
      // profile and the assertions below can distinguish "swapped to
      // profile-0" from "stayed on profile-1".
      await profilesController.setActiveProfile('profile-1');
      final telepathy = _RecordingTelepathy();
      final stateController = StateController();

      await tester.pumpProfileSettingsWithAll(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      expect(
        tester
            .widget<Button>(find.widgetWithText(Button, 'Set Active').first)
            .disabled,
        isFalse,
        reason: 'idle controller must leave the button enabled',
      );

      await tester.tap(find.widgetWithText(Button, 'Set Active').first);
      await tester.pumpAndSettle();

      expect(telepathy.identitySwitchCalls, hasLength(1),
          reason: 'profile switch must go through the atomic backend op');
      expect(telepathy.identityCalls, isEmpty,
          reason: 'setIdentity must not be called separately; it races '
              'with start_call between validation and identity mutation');
      expect(telepathy.restartManagerCalls, isEmpty,
          reason: 'restartManager must not be called separately; the atomic '
              'op owns the slot across mutation and restart');
      expect(profilesController.setActiveCalls, ['profile-1', 'profile-0'],
          reason: 'frontend active-profile change must be committed only '
              'after switchIdentityAndRestartManager succeeds; the initial '
              '"profile-1" is the test setup, the trailing "profile-0" is '
              'the swap under test');
    });

    testWidgets(
        'when the backend atomic op fails, "Set Active" must not commit '
        'the frontend active-profile change', (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Stays Active');
      await profilesController.createProfile('Cannot Switch');
      await profilesController.setActiveProfile('profile-0');
      final telepathy = _RecordingTelepathy()
        ..identitySwitchException = Exception('call slot busy');
      final stateController = StateController();

      await tester.pumpProfileSettingsWithAll(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      await tester.tap(find.widgetWithText(Button, 'Set Active').first);
      await tester.pumpAndSettle();

      expect(telepathy.identitySwitchCalls, hasLength(1),
          reason: 'the atomic backend op must be attempted');
      expect(profilesController.setActiveCalls, ['profile-0'],
          reason: 'only the test setup call should be recorded; the swap '
              'under test must not call setActiveProfile when the backend '
              'rejects the identity change');
      expect(profilesController.activeProfile, 'profile-0',
          reason: 'the original active profile must remain active');
    });
  });

  group('ProfileSettings "Delete Profile" gate', () {
    // Regression: previously delete was fire-and-forget — it called
    // `removeProfile()` and closed the dialog without ever synchronizing the
    // backend identity. If the deleted profile was the active one, the
    // frontend's new active profile had no matching signing key inside the
    // Rust backend and every subsequent call would sign with the wrong
    // identity. Worse, the button was never gated by `blockAudioChanges`, so
    // the swap could race an in-flight call.

    testWidgets(
        'the Delete button is disabled for the active profile while a call '
        'is in flight', (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Active Profile');
      await profilesController.createProfile('Idle Profile');
      await profilesController.setActiveProfile('profile-0');
      final stateController = StateController();
      final room = _roomFixture('delete-gate-pending');
      final attempt = stateController.setPendingRoom(room);
      stateController.promotePendingCallAttempt(attempt);

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: _RecordingTelepathy(),
      );

      // First profile row is active. Its delete IconButton must be disabled.
      final deleteButtons = find.byWidgetPredicate(
        (widget) => widget is IconButton && widget.tooltip == 'Delete Profile',
      );
      final activeRowDelete = deleteButtons.first;
      final activeWidget = tester.widget<IconButton>(activeRowDelete);
      expect(
        activeWidget.onPressed,
        isNull,
        reason: 'deleting the active profile during a call would race the '
            'in-flight slot with the atomic identity swap; the button must '
            'be disabled',
      );

      // The non-active profile's delete button stays enabled: removing it
      // touches neither the call slot nor the active identity, so no atomic
      // backend op is required.
      final idleRowDelete = deleteButtons.at(1);
      final idleWidget = tester.widget<IconButton>(idleRowDelete);
      expect(
        idleWidget.onPressed,
        isNotNull,
        reason: 'deleting a non-active profile during a call is safe and '
            'must remain permitted',
      );
    });

    testWidgets(
        'deleting the active profile synchronizes the replacement identity '
        'through the atomic backend op before removing it',
        (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Primary');
      await profilesController.createProfile('Secondary');
      // The fake makes the first profile active on its own; flip to the
      // first so deletion of "Primary" leaves "Secondary" as replacement.
      await profilesController.setActiveProfile('profile-0');
      final telepathy = _RecordingTelepathy();
      final stateController = StateController();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      await tester.tap(find.byTooltip('Delete Profile').first);
      await tester.pumpAndSettle();

      // Before the atomic op finishes, neither removeProfile nor the dialog
      // close should have happened.
      expect(find.text('Delete Profile'), findsOneWidget,
          reason: 'confirmation dialog must stay open while the atomic op '
              'is in flight; previously it closed immediately, leaving the '
              'user free to start a call on a stale identity');

      await tester.tap(find.widgetWithText(Button, 'Delete'));
      await tester.pumpAndSettle();

      expect(telepathy.identitySwitchCalls, hasLength(1),
          reason: 'deleting the active profile must push the replacement '
              'keypair through switchIdentityAndRestartManager');
      expect(
        telepathy.identitySwitchCalls.single,
        equals(profilesController.profiles['profile-1']!.keypair),
        reason: 'the atomic op must be called with the replacement profile '
            'keypair, not the deleted one',
      );
      expect(profilesController.profiles, contains('profile-1'));
      expect(profilesController.profiles, isNot(contains('profile-0')));
      expect(profilesController.activeProfile, 'profile-1',
          reason: 'after deletion the controller must promote the same '
              'replacement that was synced into the backend');
      expect(find.text('Delete Profile'), findsNothing,
          reason: 'confirmation dialog must close only after the frontend '
              'commit succeeds');
    });

    testWidgets(
        'a mid-switch call race: when the atomic backend op fails during a '
        'delete of the active profile, removeProfile is not called and the '
        'active profile is restored to its prior value',
        (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Active Profile');
      await profilesController.createProfile('Replacement');
      // Make the first profile active so it is the one we delete.
      await profilesController.setActiveProfile('profile-0');
      final telepathy = _RecordingTelepathy()
        ..identitySwitchException =
            Exception('call slot busy: mid-switch race');
      final stateController = StateController();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      await tester.tap(find.byTooltip('Delete Profile').first);
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(Button, 'Delete'));
      await tester.pumpAndSettle();

      expect(telepathy.identitySwitchCalls, hasLength(1),
          reason: 'the atomic backend op must be attempted before the '
              'frontend commits the deletion');
      expect(profilesController.profiles, contains('profile-0'),
          reason: 'the active profile must still exist when the backend '
              'rejects the identity swap; otherwise the UI would have no '
              'profile to match the identity the backend is still running');
      expect(profilesController.activeProfile, 'profile-0',
          reason: 'the frontend active profile must be restored to the '
              'pre-delete value so it stays consistent with the backend '
              'identity');
    });

    testWidgets(
        'deleting a non-active profile does not invoke the atomic backend op',
        (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Active');
      await profilesController.createProfile('Idle B');
      await profilesController.createProfile('Idle C');
      await profilesController.setActiveProfile('profile-0');
      final telepathy = _RecordingTelepathy();
      final stateController = StateController();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      // Delete the third profile (index 2). It is not active, so no atomic
      // backend op should run.
      await tester.tap(find.byTooltip('Delete Profile').at(2));
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(Button, 'Delete'));
      await tester.pumpAndSettle();

      expect(telepathy.identitySwitchCalls, isEmpty,
          reason: 'non-active deletion does not change the backend identity, '
              'so the atomic op must not fire');
      expect(profilesController.profiles, isNot(contains('profile-2')));
      expect(profilesController.activeProfile, 'profile-0');
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
  final List<String> removedIds = <String>[];
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

  @override
  Future<void> removeProfile(String id) async {
    // The real controller persists through `FlutterSecureStorage` /
    // `SharedPreferencesAsync`; in tests those calls are flaky and not
    // relevant to the widget behaviour under test. Mutate the in-memory
    // map the way the real controller would and pick the same fallback
    // active profile so the frontend mirrors production behaviour.
    if (!profiles.containsKey(id)) {
      return;
    }
    final wasActive = activeProfile == id;
    profiles.remove(id);
    removedIds.add(id);
    if (profiles.isEmpty) {
      final replacementId = await createProfile('Default');
      await setActiveProfile(replacementId);
    } else if (wasActive || !profiles.containsKey(activeProfile)) {
      await setActiveProfile(profiles.keys.first);
    }
    notifyListeners();
  }
}

/// Records `setIdentity`, `restartManager`, and the atomic
/// `switchIdentityAndRestartManager` so the profile-switch tests can verify
/// the gate funnels every active-profile mutation through the atomic op
/// instead of the racing separate calls.
class _RecordingTelepathy implements Telepathy {
  final List<List<int>> identityCalls = <List<int>>[];
  final List<void> restartManagerCalls = <void>[];
  final List<List<int>> identitySwitchCalls = <List<int>>[];

  /// When set, the next `switchIdentityAndRestartManager` call throws this
  /// exception to simulate the backend rejecting the swap (e.g. because the
  /// call slot is non-idle).
  Object? identitySwitchException;

  @override
  Future<void> setIdentity({required List<int> key}) async {
    identityCalls.add(List<int>.unmodifiable(key));
  }

  @override
  Future<void> restartManager() async {
    restartManagerCalls.add(null);
  }

  @override
  Future<void> switchIdentityAndRestartManager({required List<int> key}) async {
    identitySwitchCalls.add(List<int>.unmodifiable(key));
    final exception = identitySwitchException;
    if (exception != null) {
      throw exception;
    }
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
  Future<void> switchIdentityAndRestartManager(
      {required List<int> key}) async {}

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
