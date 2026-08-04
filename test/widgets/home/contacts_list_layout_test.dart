import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/widgets/contacts/contact_widget.dart';
import 'package:telepathy/widgets/contacts/contacts_list.dart';

import '../../support/fake_contact.dart';
import 'support/layout_harness.dart';

/// Contacts-list layout regressions from
/// https://github.com/chanderlud/telepathy/issues/58 and review follow-ups.
///
/// Pins: whole-number item counts per geometry (no fat/clipped rows),
/// spinner size, header/pill behavior, and flush-right row buttons.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(Harness.loadAppFont);

  /// Measures the contacts list viewport and each rendered contact row.
  /// Returns (listViewHeight, [rowHeights]).
  (double, List<double>) measureList(WidgetTester tester) {
    final listRect = tester.getRect(find.byType(ListView).first);
    final rowHeights = tester
        .elementList(find.byType(ContactWidget))
        .map((e) => tester.getRect(find.byWidget(e.widget)).height)
        .toList();
    return (listRect.height, rowHeights);
  }

  testWidgets(
      'wide compact height shows 3 rows that exactly fill the '
      'viewport (no fat rows)', (WidgetTester tester) async {
    // Regression: the wide + compact-height layout dropped to 2 items via
    // the compact flag even though its taller cap fits 3, stretching each
    // row to fill the space.
    final harness = await Harness.create();
    harness.addContact(FakeContact(id: 'c1', contactNickname: 'Profile 3'));
    harness
        .addContact(FakeContact(id: 'c2', contactNickname: 'Default Profile'));
    harness.addContact(FakeContact(id: 'c3', contactNickname: 'Contact Four'));

    setCanvasSize(tester, const Size(1002, 620));
    await pumpApp(tester, harness);

    final (listHeight, rowHeights) = measureList(tester);
    expect(rowHeights.length, 3,
        reason: '3 contacts should be rendered in the viewport');
    for (final height in rowHeights) {
      expect(height * 3, closeTo(listHeight, 1.0),
          reason: 'rows must be exactly 1/3 of the viewport, not stretched');
    }
  });

  testWidgets(
      'narrow compact height shows exactly 2 rows filling the '
      'viewport', (WidgetTester tester) async {
    final harness = await Harness.create();
    harness.addContact(FakeContact(id: 'c1', contactNickname: 'Profile 3'));
    harness
        .addContact(FakeContact(id: 'c2', contactNickname: 'Default Profile'));
    harness.addContact(FakeContact(id: 'c3', contactNickname: 'Contact Four'));

    setCanvasSize(tester, const Size(605, 600));
    await pumpApp(tester, harness);

    final (listHeight, rowHeights) = measureList(tester);
    expect(rowHeights.length, 2);
    for (final height in rowHeights) {
      expect(height * 2, closeTo(listHeight, 1.0),
          reason: 'rows must be exactly 1/2 of the viewport');
    }
  });

  testWidgets('tall narrow layout shows 3 rows with no clipping',
      (WidgetTester tester) async {
    // Regression: the third row was clipped when itemExtent was floored.
    final harness = await Harness.create();
    harness.addContact(FakeContact(id: 'c1', contactNickname: 'Profile 3'));
    harness
        .addContact(FakeContact(id: 'c2', contactNickname: 'Default Profile'));
    harness.addContact(FakeContact(id: 'c3', contactNickname: 'Contact Four'));

    setCanvasSize(tester, const Size(605, 892));
    await pumpApp(tester, harness);

    final (listHeight, rowHeights) = measureList(tester);
    expect(rowHeights.length, 3);
    for (final height in rowHeights) {
      expect(height * 3, closeTo(listHeight, 1.0),
          reason: 'rows must be exactly 1/3 of the viewport');
    }
  });

  testWidgets(
      'connecting spinner keeps its size at the narrow layout '
      'breakpoint', (WidgetTester tester) async {
    // Issue #58 case 3: 620px canvas picks the narrow branch from the
    // padded width while MediaQuery still reports "wide" — a mismatch that
    // used to leave the spinner row only 34px tall.
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness
        .addContact(FakeContact(id: 'c2', contactNickname: 'Default Profile'));
    harness.stateController
        .updateSession((contact.peerId(), const SessionStatus.connecting()));

    setCanvasSize(tester, const Size(620, 800));
    await pumpApp(tester, harness);

    final spinner = find.byType(CircularProgressIndicator);
    expect(spinner, findsWidgets);
    for (final element in tester.elementList(spinner)) {
      expect(tester.getSize(find.byWidget(element.widget)), const Size(20, 20),
          reason: 'the session spinner must not be squished');
    }
  });

  testWidgets(
      'semi-narrow two-column layout with an active call does not '
      'overflow and keeps buttons flush right', (WidgetTester tester) async {
    // Issue #58 case 1: the contact row and header overflowed on the right.
    final harness = await Harness.create();
    final contact = FakeContact(id: 'contact-1', contactNickname: 'Profile 2');
    harness.addContact(contact);
    harness.addRoom(
        Room(id: 'room-1', peerIds: const ['p1'], nickname: 'test room'));
    harness.stateController.updateSession((
      contact.peerId(),
      const SessionStatus.connected(relayed: true, remoteAddress: 'usw1-1'),
    ));
    harness.startCallWith(contact);

    setCanvasSize(tester, const Size(807, 910));
    await pumpApp(tester, harness);

    expectRowButtonsFlushRight(tester);
  });

  testWidgets(
      'tightest two-column width with a failed session manager does '
      'not overflow and collapses the pill label', (WidgetTester tester) async {
    final harness = await Harness.create();
    final contact = FakeContact(
        id: 'contact-1', contactNickname: 'A Very Long Profile Nickname');
    harness.addContact(contact);
    harness.stateController.updateSession((
      contact.peerId(),
      const SessionStatus.connected(
          relayed: true, remoteAddress: 'a-quite-long-relay-address-1'),
    ));
    harness.startCallWith(contact);
    harness.stateController.setSessionManager(ManagerState.failed);

    // The default flutter_test font has wider metrics than the bundled UI
    // font; 700px here exercises the same stress as ~660px in the app.
    setCanvasSize(tester, const Size(700, 900));
    await pumpApp(tester, harness);

    expect(find.text('Session Manager'), findsNothing,
        reason: 'the pill must drop its label before it can overflow');
    expectRowButtonsFlushRight(tester);
  });
}

/// Asserts every end-call button in the contacts list sits flush against
/// its row's right padding (card padding 12 + item margin 6 + item
/// padding 10 = 28).
void expectRowButtonsFlushRight(WidgetTester tester) {
  final cardRight = tester.getRect(find.byType(ContactsList)).right;
  final buttons = find.ancestor(
      of: find.bySemanticsLabel('End call icon'),
      matching: find.byType(IconButton));
  expect(buttons, findsWidgets);
  for (final element in tester.elementList(buttons)) {
    final buttonRect = tester.getRect(find.byWidget(element.widget));
    expect(cardRight - 28 - buttonRect.right, lessThanOrEqualTo(1.5),
        reason: 'row buttons must not float mid-row');
  }
}
