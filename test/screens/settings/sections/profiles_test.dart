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

  testWidgets('pressing Enter in create dialog creates a profile',
      (WidgetTester tester) async {
    final profilesController = FakeProfilesController();

    await tester.pumpProfileSettings(profilesController: profilesController);
    await tester.openCreateProfileDialog();
    await tester.enterText(find.byType(TextField), 'Keyboard Profile');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();

    expect(find.text('Create Profile'), findsNothing);
    expect(find.text('Keyboard Profile'), findsOneWidget);
    expect(profilesController.createdNames, <String>['Keyboard Profile']);
  });

  testWidgets('Create button rejects empty profile names',
      (WidgetTester tester) async {
    final profilesController = FakeProfilesController();

    await tester.pumpProfileSettings(profilesController: profilesController);
    await tester.openCreateProfileDialog();
    await tester.tap(find.widgetWithText(Button, 'Create'));
    await tester.pumpAndSettle();

    expect(find.text('Profile name is required.'), findsOneWidget);
    expect(profilesController.createdNames, isEmpty);
  });

  testWidgets('create dialog rejects duplicate profile names',
      (WidgetTester tester) async {
    final profilesController = FakeProfilesController()..addProfile('Existing');

    await tester.pumpProfileSettings(profilesController: profilesController);
    await tester.openCreateProfileDialog();
    await tester.enterText(find.byType(TextField), 'existing');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();

    expect(
      find.text('A profile named "existing" already exists.'),
      findsOneWidget,
    );
    expect(profilesController.createdNames, isEmpty);
  });

  testWidgets('Set Active stays blocked while audio changes are blocked',
      (WidgetTester tester) async {
    final profilesController = FakeProfilesController()..addProfile('Alpha');
    profilesController.addProfile('Beta');
    final stateController = StateController();

    await tester.pumpProfileSettings(
      profilesController: profilesController,
      stateController: stateController,
    );

    stateController.setPendingRoom(_roomFixture('connecting'));
    await tester.pump();

    final setActive = find.widgetWithText(Button, 'Set Active');
    expect(tester.widget<Button>(setActive).disabled, isTrue);

    await tester.tap(setActive);
    await tester.pump();

    expect(profilesController.switchActiveCalls, isEmpty);
  });

  testWidgets(
      'active profile deletion stays blocked while audio changes are blocked',
      (WidgetTester tester) async {
    final profilesController = FakeProfilesController()..addProfile('Alpha');
    profilesController.addProfile('Beta');
    final stateController = StateController();
    stateController.setInAudioTest(true);

    await tester.pumpProfileSettings(
      profilesController: profilesController,
      stateController: stateController,
    );

    final deleteButtons = find.byWidgetPredicate(
      (widget) => widget is IconButton && widget.tooltip == 'Delete Profile',
    );
    expect(tester.widget<IconButton>(deleteButtons.first).onPressed, isNull);
    expect(tester.widget<IconButton>(deleteButtons.at(1)).onPressed, isNotNull);
  });

  testWidgets('pending operation disables profile mutation controls',
      (WidgetTester tester) async {
    final profilesController = FakeProfilesController()..addProfile('Alpha');
    profilesController.addProfile('Beta');

    await tester.pumpProfileSettings(profilesController: profilesController);

    profilesController.setPending(true);
    await tester.pump();

    expect(
      tester.widget<Button>(find.widgetWithText(Button, 'Set Active')).disabled,
      isTrue,
    );
    final deleteButtons = find.byWidgetPredicate(
      (widget) => widget is IconButton && widget.tooltip == 'Delete Profile',
    );
    expect(tester.widget<IconButton>(deleteButtons.first).onPressed, isNull);
    expect(tester.widget<IconButton>(deleteButtons.at(1)).onPressed, isNull);
    expect(
      tester
          .widget<IconButton>(find.byWidgetPredicate(
            (widget) =>
                widget is IconButton && widget.tooltip == 'Create Profile',
          ))
          .onPressed,
      isNull,
    );
  });

  testWidgets('Set Active calls only switchActiveProfile',
      (WidgetTester tester) async {
    final profilesController = FakeProfilesController()..addProfile('Alpha');
    profilesController.addProfile('Beta');

    await tester.pumpProfileSettings(profilesController: profilesController);

    await tester.tap(find.widgetWithText(Button, 'Set Active'));
    await tester.pumpAndSettle();

    expect(profilesController.switchActiveCalls, <String>['profile-1']);
    expect(profilesController.removeCalls, isEmpty);
  });

  testWidgets('uncommitted deletion failure remains an ordinary dialog error',
      (WidgetTester tester) async {
    final profilesController = FakeProfilesController()
      ..addProfile('Alpha')
      ..removeError = StateError('index write failed');

    await tester.pumpProfileSettings(profilesController: profilesController);

    await tester.tap(find.byTooltip('Delete Profile'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(Button, 'Delete'));
    await tester.pumpAndSettle();

    expect(profilesController.removeCalls, <String>['profile-0']);
    expect(
      find.text('Could not delete profile. Please try again.'),
      findsOneWidget,
    );
    expect(find.text('Retry Cleanup'), findsNothing);
    expect(find.text('Retry'), findsNothing);
    expect(find.textContaining('Details:'), findsNothing);
  });

  testWidgets(
      'committed deletion cleanup failure closes dialog with startup retry notice',
      (WidgetTester tester) async {
    final profilesController = FakeProfilesController()
      ..addProfile('Alpha')
      ..removeCommitsBeforeFailure = true
      ..removeError = StateError('secure storage unavailable');

    await tester.pumpProfileSettings(profilesController: profilesController);

    await tester.tap(find.byTooltip('Delete Profile'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(Button, 'Delete'));
    await tester.pumpAndSettle();

    expect(profilesController.removeCalls, <String>['profile-0']);
    expect(find.text('Delete Profile'), findsNothing);
    expect(
      find.text('Profile deleted. Cleanup will retry at next startup.'),
      findsOneWidget,
    );
    expect(find.text('Retry Cleanup'), findsNothing);
  });
}

Room _roomFixture(String id) => Room(
      id: id,
      peerIds: <String>[],
      nickname: 'Room $id',
    );

extension on WidgetTester {
  Future<void> pumpProfileSettings({
    required FakeProfilesController profilesController,
    StateController? stateController,
  }) {
    return pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<ProfilesController>.value(
            value: profilesController,
          ),
          ChangeNotifierProvider<StateController>.value(
            value: stateController ?? StateController(),
          ),
          Provider<Telepathy>.value(value: _FakeTelepathy()),
        ],
        child: const MaterialApp(home: Scaffold(body: ProfileSettings())),
      ),
    );
  }

  Future<void> openCreateProfileDialog() async {
    await tap(find.byTooltip('Create Profile'));
    await pumpAndSettle();
    expect(find.text('Create Profile'), findsOneWidget);
  }
}

class FakeProfilesController extends ProfilesController {
  FakeProfilesController()
      : super(
          storage: const FlutterSecureStorage(),
          options: SharedPreferencesAsync(),
          roomHasher: ({required List<String> peers}) => peers.join('|'),
        );

  final List<String> switchActiveCalls = <String>[];
  final List<String> removeCalls = <String>[];
  final List<String> createdNames = <String>[];
  String _activeProfile = '';
  bool _pending = false;
  int _nextProfileId = 0;
  Object? removeError;
  bool removeCommitsBeforeFailure = false;

  @override
  String get activeProfile => _activeProfile;

  @override
  bool get isIdentitySwitchPending => _pending;

  void addProfile(String nickname) {
    final id = 'profile-${_nextProfileId++}';
    profiles[id] = Profile(
      id: id,
      nickname: nickname,
      peerId: 'peer-$id',
      keypair: List<int>.filled(32, _nextProfileId),
      contacts: <String, Contact>{},
      rooms: <String, Room>{},
    );
    _activeProfile = _activeProfile.isEmpty ? id : _activeProfile;
    notifyListeners();
  }

  void setPending(bool value) {
    _pending = value;
    notifyListeners();
  }

  @override
  Future<String> createProfile(String nickname) async {
    createdNames.add(nickname);
    addProfile(nickname);
    return 'profile-${_nextProfileId - 1}';
  }

  @override
  Future<void> switchActiveProfile(
    String id, {
    required Telepathy telepathy,
  }) async {
    switchActiveCalls.add(id);
    _activeProfile = id;
    notifyListeners();
  }

  @override
  Future<void> removeProfile(String id, {required Telepathy telepathy}) async {
    removeCalls.add(id);
    if (removeCommitsBeforeFailure) {
      profiles.remove(id);
      notifyListeners();
    }
    final error = removeError;
    if (error != null) {
      throw error;
    }
    profiles.remove(id);
    if (_activeProfile == id && profiles.isNotEmpty) {
      _activeProfile = profiles.keys.first;
    }
    notifyListeners();
  }
}

class _FakeTelepathy implements Telepathy {
  @override
  Object? noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}
