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
}

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
  Future<void> joinRoom({required List<String> memberStrings}) async {}

  @override
  Future<(List<AudioDevice>, List<AudioDevice>)> listDevices() async =>
      (<AudioDevice>[], <AudioDevice>[]);

  @override
  void pauseStatistics() {}

  @override
  Future<void> restartManager() async {}

  @override
  void resumeStatistics() {}

  @override
  Future<void> sendChat({required ChatMessage message}) async {}

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
  Future<void> startCall({required Contact contact}) async {}

  @override
  Future<void> startManager() async {}

  @override
  Future<void> startScreenshare({required Contact contact}) async {}

  @override
  Future<void> startSession({required Contact contact}) async {}

  @override
  Future<void> stopSession({required Contact contact}) async {}

  @override
  void setContactOutputVolume({required Contact contact}) {}
}
