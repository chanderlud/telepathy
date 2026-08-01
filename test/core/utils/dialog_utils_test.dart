import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/core/utils/dialog_utils.dart';

import '../../support/fake_contact.dart';

void main() {
  testWidgets(
      'backend cancellation cannot let an incoming-call timeout pop the original scaffold',
      (WidgetTester tester) async {
    final navigatorKey = GlobalKey<NavigatorState>();
    const homeKey = ValueKey<String>('incoming-call-race-home');
    final cancellation = Completer<void>();

    await tester.pumpWidget(
      MaterialApp(
        navigatorKey: navigatorKey,
        home: const Scaffold(
          key: homeKey,
          body: Text('Telepathy home'),
        ),
      ),
    );

    final prompt = acceptCallPrompt(
      navigatorKey.currentContext!,
      FakeContact(
        id: 'incoming-call-peer-57',
        contactNickname: 'Nora Garcia',
      ),
      cancellation.future,
    );
    await tester.pump();
    expect(find.text('Accept call from Nora Garcia?'), findsOneWidget);

    cancellation.complete();
    await tester.pump();
    expect(find.text('Accept call from Nora Garcia?'), findsNothing);

    await tester.pump(const Duration(seconds: 10));
    await tester.pumpAndSettle();

    expect(await prompt, isFalse);
    expect(find.byKey(homeKey), findsOneWidget,
        reason: 'the prompt timeout must not pop the original navigator route');
  });

  testWidgets(
      'backend cancellation removes the incoming prompt beneath a newer dialog',
      (WidgetTester tester) async {
    final navigatorKey = GlobalKey<NavigatorState>();
    const homeKey = ValueKey<String>('stacked-dialog-race-home');
    final cancellation = Completer<void>();

    await tester.pumpWidget(
      MaterialApp(
        navigatorKey: navigatorKey,
        home: const Scaffold(
          key: homeKey,
          body: Text('Telepathy home'),
        ),
      ),
    );

    final prompt = acceptCallPrompt(
      navigatorKey.currentContext!,
      FakeContact(
        id: 'incoming-call-peer-stacked-57',
        contactNickname: 'Mateo Silva',
      ),
      cancellation.future,
    );
    await tester.pump();

    final newerDialog = showDialog<void>(
      context: navigatorKey.currentContext!,
      barrierDismissible: false,
      builder: (BuildContext context) => AlertDialog(
        title: const Text('Newer dialog'),
        actions: <Widget>[
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Close newer dialog'),
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Accept call from Mateo Silva?'), findsOneWidget);
    expect(find.text('Newer dialog'), findsOneWidget);

    cancellation.complete();
    await tester.pumpAndSettle();

    expect(find.text('Accept call from Mateo Silva?'), findsNothing,
        reason: 'cancellation must remove the incoming prompt route itself');
    expect(find.text('Newer dialog'), findsOneWidget,
        reason: 'cancellation must not dismiss a newer dialog route');
    expect(await prompt, isFalse);

    await tester.tap(find.text('Close newer dialog'));
    await tester.pumpAndSettle();
    await newerDialog;
    expect(find.byKey(homeKey), findsOneWidget);
  });
}
