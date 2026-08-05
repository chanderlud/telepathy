import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/widgets/contacts/contacts_list.dart';
import 'package:telepathy/widgets/home/home_tab_view.dart';

import '../../support/fake_contact.dart';
import 'support/layout_harness.dart';

/// Home-page branch and breakpoint regressions.
///
/// Pins: the narrow/wide branch decision at the 600px padded-width
/// breakpoint, and the compact-cap behavior around the 620–640px canvas
/// window where the padded LayoutBuilder width and the unpadded MediaQuery
/// width disagree.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(Harness.loadAppFont);

  testWidgets('canvas 639 uses the narrow branch, 641 uses the wide branch',
      (WidgetTester tester) async {
    final harness = await Harness.create();
    harness
        .addContact(FakeContact(id: 'c1', contactNickname: 'Default Profile'));

    // 639 - 40px page padding = 599 < 600 breakpoint -> narrow (tab view).
    setCanvasSize(tester, const Size(639, 900));
    await pumpApp(tester, harness);
    expect(find.byType(HomeTabView), findsOneWidget);

    // 641 - 40 = 601 > 600 -> wide (side-by-side, no tab view).
    setCanvasSize(tester, const Size(641, 900));
    await pumpApp(tester, harness);
    expect(find.byType(HomeTabView), findsNothing);
  });

  testWidgets(
      'narrow branch at the width mismatch applies the compact '
      'contacts cap', (WidgetTester tester) async {
    // 620x400: narrow branch (580 padded) but MediaQuery reports a "wide"
    // 620 — the mismatch previously applied the non-compact 250px cap and
    // crushed the tab view below.
    final harness = await Harness.create();
    harness
        .addContact(FakeContact(id: 'c1', contactNickname: 'Default Profile'));

    setCanvasSize(tester, const Size(620, 400));
    await pumpApp(tester, harness);

    expect(tester.getRect(find.byType(ContactsList)).height, 170,
        reason: 'compact heights in the narrow branch use the 170px cap');
  });

  testWidgets(
      'narrow branch at the width mismatch uses the non-compact '
      'cap at taller heights', (WidgetTester tester) async {
    final harness = await Harness.create();
    harness
        .addContact(FakeContact(id: 'c1', contactNickname: 'Default Profile'));

    setCanvasSize(tester, const Size(620, 900));
    await pumpApp(tester, harness);

    expect(tester.getRect(find.byType(ContactsList)).height, 250,
        reason: 'non-compact heights in the narrow branch use the 250px cap');
  });
}
