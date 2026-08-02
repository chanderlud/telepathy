import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/widgets/call/call_details_widget.dart';
import 'package:telepathy/widgets/common/audio_level.dart';
import 'package:telepathy/widgets/common/gradient_chart.dart';

import '../../support/fake_contact.dart';
import 'support/layout_harness.dart';

/// Call-details card layout regressions from
/// https://github.com/chanderlud/telepathy/issues/58 (case 4).
///
/// Pins: no vertical overflow at either top-section cap, and the loss
/// chart's show/hide threshold.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(Harness.loadAppFont);

  Future<Harness> activeCallHarness() async {
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness.stateController.updateSession((
      contact.peerId(),
      const SessionStatus.connected(relayed: true, remoteAddress: 'usw1-1'),
    ));
    harness.startCallWith(contact);
    return harness;
  }

  void expectDetailsComplete(WidgetTester tester) {
    // Guard against vacuous passes: the card must be on screen for the
    // overflow checks to mean anything.
    expect(find.byType(CallDetailsWidget), findsOneWidget);
    // Title, both level meters, and the stats row must all be present.
    expect(find.text('Input level'), findsOneWidget);
    expect(find.text('Output level'), findsOneWidget);
    expect(find.byType(AudioLevel), findsNWidgets(2));
    expect(find.bySemanticsLabel('Latency icon'), findsOneWidget);
    expect(find.bySemanticsLabel('Upload icon'), findsOneWidget);
    expect(find.bySemanticsLabel('Download icon'), findsOneWidget);
  }

  testWidgets('wide tall layout renders the full card including the chart',
      (WidgetTester tester) async {
    final harness = await activeCallHarness();
    setCanvasSize(tester, const Size(807, 910));
    await pumpApp(tester, harness);

    expectDetailsComplete(tester);
    expect(find.byType(GradientMiniLineChart), findsOneWidget);
  });

  testWidgets(
      'case 4: short wide layout renders the full card without '
      'overflow', (WidgetTester tester) async {
    final harness = await activeCallHarness();
    setCanvasSize(tester, const Size(1002, 848));
    await pumpApp(tester, harness);

    expectDetailsComplete(tester);
    expect(find.byType(GradientMiniLineChart), findsOneWidget);
  });

  testWidgets(
      'case 4b: compact wide layout hides the chart but keeps '
      'levels and stats', (WidgetTester tester) async {
    final harness = await activeCallHarness();
    setCanvasSize(tester, const Size(1002, 620));
    await pumpApp(tester, harness);

    expectDetailsComplete(tester);
    expect(find.byType(GradientMiniLineChart), findsNothing,
        reason: 'the chart must be the element dropped when space is tight');
  });
}
