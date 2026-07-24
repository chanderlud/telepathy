import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/frb_generated.dart'
    show RustLib, RustLibApi;
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/screens/settings/sections/profiles.dart';
import 'package:telepathy/widgets/common/index.dart';

void main() {
  setUp(() {
    FlutterSecureStorage.setMockInitialValues(<String, String>{});
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
    //
    // The two-phase transaction adds a second gate,
    // `ProfilesController.isIdentitySwitchPending`, that covers the in-flight
    // transaction itself so a second switch cannot begin while the first is
    // still committing or cancelling across both layers.

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

      expect(profilesController.switchActiveCalls, isEmpty,
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
      expect(profilesController.switchActiveCalls, isEmpty);
    });

    testWidgets(
        'the "Set Active" button is disabled while an identity-switch '
        'transaction is in flight', (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Pending A');
      await profilesController.createProfile('Pending B');
      await profilesController.simulateStartupWithActive('profile-0');
      final stateController = StateController();
      final telepathy = _RecordingTelepathy()..pauseBeginIdentitySwitch();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      final switchFuture = profilesController.switchActiveProfile(
        'profile-1',
        telepathy: telepathy,
      );
      await telepathy.beginIdentitySwitchEntered.future;
      await tester.pump();

      final setActiveButton = find.widgetWithText(Button, 'Set Active');
      expect(setActiveButton, findsOneWidget);
      expect(tester.widget<Button>(setActiveButton).disabled, isTrue,
          reason: 'the in-flight transaction must block re-entrant switches; '
              'a second begin_identity_switch would either fail to acquire '
              'the slot or wedge the backend');

      await tester.tap(setActiveButton);
      await tester.pump();

      expect(profilesController.switchActiveCalls, isEmpty,
          reason: 'the defensive early return inside onPressed must reject '
              'mutations while isIdentitySwitchPending is true');
      telepathy.releaseBeginIdentitySwitch();
      await switchFuture;
    });

    testWidgets(
        'when the controller is idle, "Set Active" runs the two-phase '
        'transaction and commits the frontend swap only after the backend '
        'accepts it', (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Idle Profile A');
      await profilesController.createProfile('Idle Profile B');
      // The fake does not auto-select the first profile; pick the second so
      // tapping the first row's "Set Active" actually changes the active
      // profile and the assertions below can distinguish "swapped to
      // profile-0" from "stayed on profile-1".
      await profilesController.simulateStartupWithActive('profile-1');
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

      expect(telepathy.beginCalls, hasLength(1),
          reason: 'profile switch must begin the two-phase transaction');
      expect(telepathy.commitCalls, hasLength(1),
          reason: 'the transaction must commit the new identity');
      expect(telepathy.cancelCalls, isEmpty,
          reason: 'no cancel when the happy path succeeds');
      expect(
        telepathy.beginCalls.single.key,
        equals(profilesController.profiles['profile-0']!.keypair),
        reason: 'begin must be called with the target profile keypair',
      );
      expect(
        telepathy.beginCalls.single.contacts,
        isEmpty,
        reason: 'the fake profile has no contacts; the snapshot must still '
            'be passed so the Rust side does not fall back to getContacts '
            'and rehydrate the wrong session set',
      );
      expect(
        profilesController.activeProfile,
        'profile-0',
        reason: 'frontend active profile must be swapped after commit',
      );
    });

    testWidgets(
        'when the backend rejects begin_identity_switch, "Set Active" must '
        'not commit the frontend active-profile change', (
      WidgetTester tester,
    ) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Stays Active');
      await profilesController.createProfile('Cannot Switch');
      await profilesController.simulateStartupWithActive('profile-0');
      final telepathy = _RecordingTelepathy()
        ..beginException = Exception('call slot busy');
      final stateController = StateController();

      await tester.pumpProfileSettingsWithAll(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      await tester.tap(find.widgetWithText(Button, 'Set Active').first);
      await tester.pumpAndSettle();

      expect(telepathy.beginCalls, hasLength(1),
          reason: 'the transaction must attempt begin');
      expect(telepathy.commitCalls, isEmpty,
          reason: 'commit must not run when begin fails');
      expect(telepathy.cancelCalls, isEmpty,
          reason: 'cancel is for post-begin pre-commit failures; begin '
              'itself failing means no slot is held, so cancel is a no-op '
              'the controller does not call');
      expect(profilesController.activeProfile, 'profile-0',
          reason: 'the original active profile must remain active');
    });

    testWidgets(
        'when commit_identity_switch fails, the frontend rolls back its '
        'active-profile change so it matches the identity Rust restored',
        (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Active');
      await profilesController.createProfile('Target');
      await profilesController.simulateStartupWithActive('profile-0');
      final telepathy = _RecordingTelepathy()
        ..commitException = Exception('manager restart failed');
      final stateController = StateController();

      await tester.pumpProfileSettingsWithAll(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      await tester.tap(find.widgetWithText(Button, 'Set Active').first);
      await tester.pumpAndSettle();

      expect(telepathy.beginCalls, hasLength(1),
          reason: 'the transaction must begin so the snapshot is captured');
      expect(telepathy.commitCalls, hasLength(1),
          reason: 'commit must be attempted after the frontend persistence '
              'succeeds; the controller cannot know commit will fail until '
              'it tries');
      expect(profilesController.activeProfile, 'profile-0',
          reason: 'commit failure must restore the frontend to its prior '
              'active profile so it stays consistent with the identity Rust '
              'rolled back to internally');
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
    //
    // The two-phase transaction additionally requires the deletion path to
    // create+persist any replacement identity BEFORE acquiring the backend
    // gate, and the in-flight gate must block sibling deletions.

    testWidgets(
        'the Delete button is disabled for the active profile while a call '
        'is in flight', (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Active Profile');
      await profilesController.createProfile('Idle Profile');
      await profilesController.simulateStartupWithActive('profile-0');
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
            'in-flight slot with the identity switch; the button must '
            'be disabled',
      );

      // The non-active profile's delete button stays enabled: removing it
      // touches neither the call slot nor the active identity, so no
      // transactional backend op is required.
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
        'all Delete buttons are disabled while an identity-switch '
        'transaction is in flight', (WidgetTester tester) async {
      // The transaction flag is shared across the whole controller: even
      // non-active deletions would mutate the profile index the active
      // transaction is reading, so the UI must block every delete while the
      // flag is set.
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Active');
      await profilesController.createProfile('Idle B');
      await profilesController.createProfile('Idle C');
      await profilesController.simulateStartupWithActive('profile-0');
      final stateController = StateController();
      final telepathy = _RecordingTelepathy()..pauseBeginIdentitySwitch();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      final switchFuture = profilesController.switchActiveProfile(
        'profile-1',
        telepathy: telepathy,
      );
      await telepathy.beginIdentitySwitchEntered.future;
      await tester.pump();

      final deleteButtons = find.byWidgetPredicate(
        (widget) => widget is IconButton && widget.tooltip == 'Delete Profile',
      );
      expect(deleteButtons, findsNWidgets(3));
      final activeDelete = tester.widget<IconButton>(deleteButtons.at(0));
      final idleDeleteB = tester.widget<IconButton>(deleteButtons.at(1));
      final idleDeleteC = tester.widget<IconButton>(deleteButtons.at(2));
      expect(activeDelete.onPressed, isNull);
      expect(idleDeleteB.onPressed, isNull,
          reason: 'even non-active deletes must be blocked while a '
              'transaction is in flight; they mutate the profile index the '
              'transaction is reading');
      expect(idleDeleteC.onPressed, isNull);
      telepathy.releaseBeginIdentitySwitch();
      await switchFuture;
    });

    testWidgets(
        'deleting the active profile commits the replacement identity '
        'through the two-phase transaction before removing it',
        (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Primary');
      await profilesController.createProfile('Secondary');
      // The fake makes the first profile active on its own; flip to the
      // first so deletion of "Primary" leaves "Secondary" as replacement.
      await profilesController.simulateStartupWithActive('profile-0');
      final telepathy = _RecordingTelepathy();
      final stateController = StateController();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      await tester.tap(find.byTooltip('Delete Profile').first);
      await tester.pumpAndSettle();

      // Before the user confirms, neither the transaction nor the dialog
      // close should have happened.
      expect(find.text('Delete Profile'), findsOneWidget,
          reason: 'confirmation dialog must stay open until the user '
              'confirms; previously it closed immediately, leaving the '
              'user free to start a call on a stale identity');

      await tester.tap(find.widgetWithText(Button, 'Delete'));
      await tester.pumpAndSettle();

      expect(telepathy.beginCalls, hasLength(1),
          reason: 'deleting the active profile must begin the transaction');
      expect(telepathy.commitCalls, hasLength(1),
          reason: 'the replacement identity must be committed');
      expect(
        telepathy.beginCalls.single.key,
        equals(profilesController.profiles['profile-1']!.keypair),
        reason: 'begin must be called with the replacement profile '
            'keypair, not the deleted one',
      );
      expect(telepathy.cancelCalls, isEmpty,
          reason: 'happy path does not cancel');
      expect(profilesController.profiles, contains('profile-1'));
      expect(profilesController.profiles, isNot(contains('profile-0')));
      expect(profilesController.activeProfile, 'profile-1',
          reason: 'after deletion the controller must promote the same '
              'replacement that was synced into the backend');
      expect(find.text('Delete Profile'), findsNothing,
          reason: 'confirmation dialog must close only after the frontend '
              'commit succeeds');
    });

    group('sole active profile deletion', () {
      final rustApi = _DeterministicRustApi();

      setUpAll(() {
        RustLib.initMock(api: rustApi);
      });

      setUp(rustApi.reset);

      tearDownAll(RustLib.dispose);

      testWidgets(
          'creates and commits a replacement before deleting the persisted '
          'active profile', (WidgetTester tester) async {
        const originalId = 'profile-alpha';
        final originalKeypair = List<int>.filled(32, 3);
        const storage = FlutterSecureStorage();
        final options = SharedPreferencesAsync();
        final profilesController = ProfilesController(
          storage: storage,
          options: options,
          roomHasher: ({required List<String> peers}) => peers.join('|'),
        );
        addTearDown(profilesController.dispose);

        await storage.write(
          key: '$originalId-keypair',
          value: base64Encode(originalKeypair),
        );
        await storage.write(
          key: '$originalId-peerId',
          value: '12D3KooWProfileAlpha00000000000000000000000000000',
        );
        await storage.write(key: '$originalId-nickname', value: 'Alpha');
        await storage.write(key: '$originalId-contacts', value: '{}');
        await storage.write(key: '$originalId-rooms', value: '{}');
        await options.setStringList('profilesV2', const <String>[originalId]);
        await options.setString('activeProfile', originalId);
        await profilesController.init(const <String>[]);

        final telepathy = _RecordingTelepathy();
        await tester.pumpProfileSettingsWithState(
          profilesController: profilesController,
          stateController: StateController(),
          telepathy: telepathy,
        );

        await tester.tap(find.byTooltip('Delete Profile'));
        await tester.pump();

        expect(find.text('Delete Profile'), findsOneWidget,
            reason: 'deletion requires explicit confirmation');

        await tester.tap(find.widgetWithText(Button, 'Delete'));
        await tester.pumpAndSettle();

        final replacementId = profilesController.profiles.keys.single;
        expect(rustApi.generateKeysCalls, 1,
            reason: 'sole-profile deletion must create a replacement keypair');
        expect(profilesController.profiles, isNot(contains(originalId)));
        expect(profilesController.activeProfile, replacementId);
        expect(profilesController.profiles[replacementId]!.nickname, 'Default');
        expect(telepathy.beginCalls, hasLength(1));
        expect(telepathy.beginCalls.single.key,
            List<int>.filled(32, _DeterministicRustApi.keyByte));
        expect(telepathy.commitCalls, hasLength(1));
        expect(telepathy.cancelCalls, isEmpty);
        expect(find.text('Delete Profile'), findsNothing,
            reason: 'dialog closes only after the replacement commit succeeds');

        expect(await options.getString('activeProfile'), replacementId);
        expect(
          await options.getStringList('profilesV2'),
          orderedEquals(<String>[replacementId]),
        );
        expect(await storage.read(key: '$originalId-keypair'), isNull);
        expect(await storage.read(key: '$originalId-peerId'), isNull);
        expect(await storage.read(key: '$originalId-nickname'), isNull);
        expect(await storage.read(key: '$originalId-contacts'), isNull);
        expect(await storage.read(key: '$originalId-rooms'), isNull);
        expect(await storage.read(key: '$replacementId-keypair'),
            base64Encode(List<int>.filled(32, _DeterministicRustApi.keyByte)));
        expect(await storage.read(key: '$replacementId-peerId'),
            '12D3KooWGeneratedPeerId000000000000000000000000000000');
        expect(await storage.read(key: '$replacementId-nickname'), 'Default');
      });
    });

    testWidgets(
        'a mid-switch race: when commit_identity_switch fails during a '
        'delete of the active profile, the deleted profile is restored and '
        'the active profile is rolled back so the UI matches the identity '
        'Rust is actually running', (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Active Profile');
      await profilesController.createProfile('Replacement');
      // Make the first profile active so it is the one we delete.
      await profilesController.simulateStartupWithActive('profile-0');
      final telepathy = _RecordingTelepathy()
        ..commitException = Exception('call slot busy: mid-switch race');
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

      expect(telepathy.beginCalls, hasLength(1),
          reason: 'the transaction must begin before the frontend commits '
              'the deletion');
      expect(telepathy.commitCalls, hasLength(1),
          reason: 'commit must be attempted; the controller cannot know it '
              'will fail until it tries');
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
        'deleting a non-active profile does not invoke the two-phase '
        'transaction', (WidgetTester tester) async {
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('Active');
      await profilesController.createProfile('Idle B');
      await profilesController.createProfile('Idle C');
      await profilesController.simulateStartupWithActive('profile-0');
      final telepathy = _RecordingTelepathy();
      final stateController = StateController();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      // Delete the third profile (index 2). It is not active, so no
      // transactional backend op should run.
      await tester.tap(find.byTooltip('Delete Profile').at(2));
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(Button, 'Delete'));
      await tester.pumpAndSettle();

      expect(telepathy.beginCalls, isEmpty,
          reason: 'non-active deletion does not change the backend '
              'identity, so the transaction must not fire');
      expect(telepathy.commitCalls, isEmpty);
      expect(telepathy.cancelCalls, isEmpty);
      expect(profilesController.profiles, isNot(contains('profile-2')));
      expect(profilesController.activeProfile, 'profile-0');
    });

    testWidgets(
        'attempting to delete the target of an in-flight switch is blocked '
        'at the UI layer by the transaction flag', (WidgetTester tester) async {
      // Scenario: the user starts a switch A -> B, then while the
      // transaction is still in flight tries to delete B. The
      // `isIdentitySwitchPending` flag must keep B's delete button disabled
      // so the deletion cannot race the commit/cancel.
      final profilesController = FakeProfilesController();
      await profilesController.createProfile('A');
      await profilesController.createProfile('B');
      await profilesController.simulateStartupWithActive('profile-0');
      final telepathy = _RecordingTelepathy();
      final stateController = StateController();

      await tester.pumpProfileSettingsWithState(
        profilesController: profilesController,
        stateController: stateController,
        telepathy: telepathy,
      );

      telepathy.pauseBeginIdentitySwitch();
      final switchFuture = profilesController.switchActiveProfile(
        'profile-1',
        telepathy: telepathy,
      );
      await telepathy.beginIdentitySwitchEntered.future;
      await tester.pump();

      // Locate IconButton widgets directly (find.byTooltip returns Tooltip
      // wrappers, which cannot be cast to IconButton).
      final deleteButtons = find.byWidgetPredicate(
        (widget) => widget is IconButton && widget.tooltip == 'Delete Profile',
      );
      final targetDeleteButton = tester.widget<IconButton>(deleteButtons.at(1));
      expect(
        targetDeleteButton.onPressed,
        isNull,
        reason: 'the target of an in-flight switch must not be deletable; '
            'its storage entries are about to be observed by the commit '
            'path and a sibling deletion would corrupt the snapshot',
      );

      // Tap it anyway; the disabled button swallows the gesture. No
      // confirmation dialog should open.
      await tester.tap(deleteButtons.at(1), warnIfMissed: false);
      await tester.pump();
      expect(find.text('Delete Profile'), findsNothing,
          reason: 'the confirmation dialog must not open when the button '
              'is disabled');
      telepathy.releaseBeginIdentitySwitch();
      await switchFuture;
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
    required ProfilesController profilesController,
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
          roomHasher: ({required List<String> peers}) => peers.join('|'),
        );

  final List<String> createdNames = <String>[];
  final List<String> switchActiveCalls = <String>[];
  final List<String> removeCalls = <String>[];
  final List<String> removedIds = <String>[];
  int _nextProfileId = 0;

  /// Realistic 32-byte identity seed so the production
  /// `begin_identity_switch` payload validation would pass even if this
  /// fake were driven against the real Rust bridge. The previous
  /// `const <int>[]` placeholder let widget tests re-implement the
  /// transaction with empty keys, hiding production-only failures (the
  /// `try_into` length check on the Rust side would have rejected every
  /// commit).
  List<int> _realisticKeypair(int seed) =>
      List<int>.generate(32, (i) => (seed + i) & 0xFF);

  /// Overrides the production `createProfile` solely to avoid calling
  /// `generateKeys`, which requires the Rust runtime not loaded in widget
  /// tests. The fake writes the profile to both the in-memory map AND
  /// `FlutterSecureStorage` so a subsequent `init()` can reload it
  /// realistically (no test-only hooks on the production class).
  @override
  Future<String> createProfile(String nickname) async {
    final id = 'profile-${_nextProfileId++}';
    createdNames.add(nickname);
    final keypair = _realisticKeypair(_nextProfileId);
    final cleanNickname =
        nickname.trim().isEmpty ? 'Unnamed Profile' : nickname;
    profiles[id] = Profile(
      id: id,
      nickname: cleanNickname,
      peerId: 'peer-$id',
      keypair: keypair,
      contacts: <String, Contact>{},
      rooms: <String, Room>{},
    );
    // Persist so init() can reload the profile from storage. The production
    // _loadProfile validates 32-byte keypairs, so the realistic seed above
    // is required for the reload to succeed.
    await storage.write(
      key: '$id-keypair',
      value: base64Encode(keypair),
    );
    await storage.write(key: '$id-peerId', value: 'peer-$id');
    await storage.write(key: '$id-nickname', value: cleanNickname);
    await options.setStringList(
      'profilesV2',
      profiles.keys.toList(growable: false),
    );
    notifyListeners();
    return id;
  }

  /// Writes the active-profile preference and re-runs `init()` so the
  /// controller's `_activeProfile` field is set through the production
  /// startup path instead of a test-only setter. Used by widget tests
  /// that need a known starting active profile. (Comment 5 removed the
  /// production `activeProfileForTesting` setter.)
  Future<void> simulateStartupWithActive(String id) async {
    await options.setString('activeProfile', id);
    await init(const <String>[]);
  }
}

/// Records each two-phase transaction call so the profile-switch tests can
/// verify the gate funnels every active-profile mutation through
/// begin/commit/cancel instead of the racing separate `setIdentity` +
/// `restartManager` calls.
class _RecordingTelepathy implements Telepathy {
  final List<_BeginRecord> beginCalls = <_BeginRecord>[];
  final List<void> commitCalls = <void>[];
  final List<void> cancelCalls = <void>[];
  final List<List<int>> identityCalls = <List<int>>[];
  final List<void> restartManagerCalls = <void>[];
  final Completer<void> beginIdentitySwitchEntered = Completer<void>();
  Completer<void>? _beginIdentitySwitchRelease;

  Object? beginException;

  Object? commitException;

  void pauseBeginIdentitySwitch() {
    _beginIdentitySwitchRelease = Completer<void>();
  }

  void releaseBeginIdentitySwitch() {
    _beginIdentitySwitchRelease!.complete();
  }

  @override
  Future<void> beginIdentitySwitch({
    required List<int> targetKey,
    required List<Contact> targetContacts,
  }) async {
    beginCalls.add(_BeginRecord(
      key: List<int>.unmodifiable(targetKey),
      contacts: List<Contact>.unmodifiable(targetContacts),
    ));
    if (!beginIdentitySwitchEntered.isCompleted) {
      beginIdentitySwitchEntered.complete();
    }
    final exception = beginException;
    if (exception != null) {
      throw exception;
    }
    await _beginIdentitySwitchRelease?.future;
  }

  @override
  Future<void> commitIdentitySwitch() async {
    commitCalls.add(null);
    final exception = commitException;
    if (exception != null) {
      throw exception;
    }
  }

  @override
  Future<void> recoverIdentitySwitch() async {}

  @override
  Future<void> cancelIdentitySwitch() async {
    cancelCalls.add(null);
  }

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

class _BeginRecord {
  _BeginRecord({required this.key, required this.contacts});

  final List<int> key;
  final List<Contact> contacts;
}

class _DeterministicRustApi implements RustLibApi {
  static const int keyByte = 7;

  int generateKeysCalls = 0;

  void reset() {
    generateKeysCalls = 0;
  }

  @override
  (String, Uint8List) crateFlutterUtilsGenerateKeys() {
    generateKeysCalls += 1;
    return (
      '12D3KooWGeneratedPeerId000000000000000000000000000000',
      Uint8List.fromList(List<int>.filled(32, keyByte)),
    );
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class FakeTelepathy implements Telepathy {
  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  Future<void> audioTest() async {}

  @override
  Future<void> beginIdentitySwitch({
    required List<int> targetKey,
    required List<Contact> targetContacts,
  }) async {}

  @override
  ChatMessage buildChat({
    required Contact contact,
    required String text,
    required List<(String, Uint8List)> attachments,
  }) {
    throw UnimplementedError();
  }

  @override
  Future<void> cancelIdentitySwitch() async {}

  @override
  Future<void> commitIdentitySwitch() async {}

  @override
  Future<void> recoverIdentitySwitch() async {}

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
