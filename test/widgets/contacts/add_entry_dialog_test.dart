import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/profiles_controller.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/core/testing/mock_backend.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/widgets/call/room_details_widget.dart';
import 'package:telepathy/widgets/contacts/add_entry_dialog.dart';

import '../../support/fake_contact.dart';
import '../home/support/layout_harness.dart';

/// The harness fake plus a working `startSession` (the add-contact submit
/// path calls it). The full [MockTelepathy] is not usable here: its simulated
/// connection delays outlive the widget tree and fail the pending-timer
/// invariant.
class _TestTelepathy extends FakeTelepathy {
  @override
  Future<void> startSession({required Contact contact}) async {}
}

/// Tests for the redesigned add contact / add room flow and the active-room
/// details panel (issue #16).
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(Harness.loadAppFont);

  Widget buildDialogApp(Harness harness) {
    return DefaultAssetBundle(
      bundle: SvgAwareAssetBundle(),
      child: MultiProvider(
        providers: [
          ChangeNotifierProvider<ProfilesController>.value(
              value: harness.profilesController),
          ChangeNotifierProvider<StateController>.value(
              value: harness.stateController),
          Provider<Telepathy>.value(value: _TestTelepathy()),
          Provider<SoundPlayer>.value(value: FakeSoundPlayer()),
        ],
        child: MaterialApp(
          theme: ThemeData(fontFamily: 'Nunito'),
          home: const Scaffold(body: Center(child: AddEntryDialog())),
        ),
      ),
    );
  }

  testWidgets('chooser offers contact and room cards', (tester) async {
    final harness = await Harness.create();
    await tester.pumpWidget(buildDialogApp(harness));

    expect(find.text('Add New'), findsOneWidget);
    expect(find.text('Contact'), findsOneWidget);
    expect(find.text('Room'), findsOneWidget);
    expect(find.text('Call one person directly'), findsOneWidget);
    expect(find.text('Group call with several peers'), findsOneWidget);
  });

  testWidgets('add contact validates duplicates inline and adds the contact',
      (tester) async {
    final harness = await Harness.create();
    harness.addContact(
        FakeContact(id: 'peer-existing', contactNickname: 'Existing'));

    await tester.pumpWidget(buildDialogApp(harness));
    await tester.tap(find.text('Contact'));
    await tester.pumpAndSettle();

    // submit stays disabled until both fields have text
    final ElevatedButton submit = tester.widget(find.byType(ElevatedButton));
    expect(submit.enabled, isFalse);

    await tester.enterText(
        find.widgetWithText(TextField, 'Nickname'), 'New Friend');
    await tester.enterText(
        find.widgetWithText(TextField, 'Peer ID'), 'peer-existing');
    await tester.pump();

    await tester.tap(find.widgetWithText(ElevatedButton, 'Add Contact'));
    await tester.pumpAndSettle();

    expect(
        find.text('A contact for this peer ID already exists'), findsOneWidget);
    expect(harness.profilesController.contacts, hasLength(1));

    // fixing the peer id clears the error and submits successfully
    await tester.enterText(
        find.widgetWithText(TextField, 'Peer ID'), 'peer-new');
    await tester.tap(find.widgetWithText(ElevatedButton, 'Add Contact'));
    await tester.pumpAndSettle();

    expect(harness.profilesController.contacts, hasLength(2));
    expect(harness.profilesController.contacts.values.last.nickname(),
        'New Friend');
  });

  testWidgets('add room builds members as chips and creates the room',
      (tester) async {
    final harness = await Harness.create();
    harness.addContact(FakeContact(id: 'peer-ada', contactNickname: 'Ada'));

    await tester.pumpWidget(buildDialogApp(harness));
    await tester.tap(find.text('Room'));
    await tester.pumpAndSettle();

    // nickname-only keeps submit disabled: members are required
    await tester.enterText(
        find.widgetWithText(TextField, 'Room name'), 'Study Group');
    await tester.pump();
    final ElevatedButton disabledSubmit =
        tester.widget(find.byType(ElevatedButton));
    expect(disabledSubmit.enabled, isFalse);

    // add a member by peer id; it shows up as a removable chip
    await tester.enterText(
        find.widgetWithText(TextField, 'Peer ID'), 'peer-bob');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pump();

    expect(find.text('peer-bob'), findsOneWidget);

    await tester.tap(find.text('Create Room'));
    await tester.pumpAndSettle();

    expect(harness.profilesController.rooms, hasLength(1));
    final Room room = harness.profilesController.rooms.values.single;
    expect(room.nickname, 'Study Group');
    // the local profile is always added as a member
    expect(room.peerIds, containsAll(['peer-bob', 'layout-peer-id']));
  });

  testWidgets('add room reports invalid clipboard contents inline',
      (tester) async {
    final harness = await Harness.create();

    await tester.pumpWidget(buildDialogApp(harness));
    await tester.tap(find.text('Room'));
    await tester.pumpAndSettle();

    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.getData') {
        return <String, dynamic>{'text': 'not room details'};
      }
      return null;
    });

    await tester.tap(find.text('Paste room details'));
    await tester.pumpAndSettle();

    expect(
        find.text('Clipboard text is not valid room details'), findsOneWidget);
  });

  testWidgets('room details shows members grouped by online state',
      (tester) async {
    final harness = await Harness.create();
    const self = 'layout-peer-id';
    final room = Room(
      id: 'room-1',
      peerIds: const [self, 'peer-ada', 'peer-grace', 'peer-offline'],
      nickname: 'Weekend Gaming',
    );
    harness.addRoom(room);
    harness.addContact(FakeContact(id: 'peer-ada', contactNickname: 'Ada'));
    harness.addContact(FakeContact(id: 'peer-grace', contactNickname: 'Grace'));
    harness
        .addContact(FakeContact(id: 'peer-offline', contactNickname: 'Oscar'));

    harness.stateController.setPendingRoom(room);
    harness.stateController
        .promotePendingCallAttempt(harness.stateController.currentCallAttempt);
    harness.stateController.roomJoin('peer-ada');
    harness.stateController.roomJoin('peer-grace');

    await tester.pumpWidget(DefaultAssetBundle(
      bundle: SvgAwareAssetBundle(),
      child: MultiProvider(
        providers: [
          ChangeNotifierProvider<ProfilesController>.value(
              value: harness.profilesController),
          ChangeNotifierProvider<StateController>.value(
              value: harness.stateController),
          Provider<Telepathy>.value(value: _TestTelepathy()),
          Provider<SoundPlayer>.value(value: FakeSoundPlayer()),
        ],
        child: MaterialApp(
          theme: ThemeData(fontFamily: 'Nunito'),
          home: const Scaffold(body: RoomDetailsWidget()),
        ),
      ),
    ));
    await tester.pumpAndSettle();

    expect(find.text('Weekend Gaming'), findsOneWidget);
    expect(find.text('3/4 online'), findsOneWidget);
    expect(find.text('3 online'), findsOneWidget);
    expect(find.text('1 offline'), findsOneWidget);
    expect(find.text('You'), findsOneWidget);
    expect(find.text('Ada'), findsOneWidget);
    expect(find.text('Grace'), findsOneWidget);
    expect(find.text('Oscar'), findsOneWidget);
  });
}
