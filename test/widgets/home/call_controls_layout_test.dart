import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/screens/home/home_page.dart';
import 'package:telepathy/widgets/call/call_controls.dart';

import '../../support/fake_contact.dart';
import 'support/layout_harness.dart';

/// Call-controls layout regressions from
/// https://github.com/chanderlud/telepathy/issues/58 (case 2).
///
/// Pins: the bottom control bar is never pushed out of the canvas, at any
/// height, in either branch.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(Harness.loadAppFont);

  void expectBottomBarVisible(WidgetTester tester, Size canvas) {
    // Every IconButton in the CallControls bottom row must be inside the
    // canvas. The bottom bar is the last row of buttons in CallControls.
    final buttons = tester
        .elementList(find.descendant(
            of: find.byType(CallControls), matching: find.byType(IconButton)))
        .toList();
    expect(buttons, isNotEmpty);
    for (final element in buttons) {
      final rect = tester.getRect(find.byWidget(element.widget));
      expect(rect.bottom, lessThanOrEqualTo(canvas.height),
          reason: 'control bar clipped by the window bottom');
      expect(rect.top, greaterThanOrEqualTo(0));
    }
  }

  testWidgets(
      'case 2: medium height narrow layout keeps the bottom bar '
      'inside the canvas', (WidgetTester tester) async {
    final harness = await Harness.create();
    harness
        .addContact(FakeContact(id: 'c1', contactNickname: 'Default Profile'));
    harness.addContact(FakeContact(id: 'c2', contactNickname: 'Profile 3'));

    setCanvasSize(tester, const Size(605, 892));
    await pumpApp(tester, harness);

    expectBottomBarVisible(tester, const Size(605, 892));
    final Rect appRect = tester.getRect(find.byType(HomePage));
    expect(appRect.bottom, lessThanOrEqualTo(892));
  });

  testWidgets(
      'compact height narrow layout with an active call does not '
      'overflow', (WidgetTester tester) async {
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness.startCallWith(contact);

    setCanvasSize(tester, const Size(605, 600));
    await pumpApp(tester, harness);

    expectBottomBarVisible(tester, const Size(605, 600));
  });

  testWidgets(
      'case 2c: short narrow layout at the width breakpoint does '
      'not overflow', (WidgetTester tester) async {
    // 620x400: the padded/MediaQuery width mismatch applied the 250px
    // contacts cap in a compact-height window, crushing the tab view and
    // overflowing CallControls by 46px.
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness.startCallWith(contact);

    setCanvasSize(tester, const Size(620, 400));
    await pumpApp(tester, harness);

    expectBottomBarVisible(tester, const Size(620, 400));
  });

  testWidgets(
      'very short heights make the sliders scroll instead of '
      'clipping the bar', (WidgetTester tester) async {
    final harness = await Harness.create();
    harness
        .addContact(FakeContact(id: 'c1', contactNickname: 'Default Profile'));

    setCanvasSize(tester, const Size(605, 500));
    await pumpApp(tester, harness);

    expect(
        find.descendant(
            of: find.byType(CallControls),
            matching: find.byType(SingleChildScrollView)),
        findsOneWidget,
        reason: 'the slider section must be scrollable as a fallback');
    expectBottomBarVisible(tester, const Size(605, 500));
  });
}
