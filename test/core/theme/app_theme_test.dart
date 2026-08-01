import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/core/theme/app_theme.dart';

void main() {
  testWidgets('switch thumb is dark gray only when selected and hovered', (
    WidgetTester tester,
  ) async {
    const fallbackThumbColor = Colors.amber;
    late ThemeData appTheme;

    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(
          tabBarTheme: const TabBarThemeData(
            indicatorColor: fallbackThumbColor,
          ),
        ),
        home: Builder(
          builder: (BuildContext context) {
            appTheme = AppTheme.dark(
              context,
              primaryColor: Colors.blue.toARGB32(),
              secondaryColor: Colors.green.toARGB32(),
            );
            return const SizedBox();
          },
        ),
      ),
    );

    final thumbColor = appTheme.switchTheme.thumbColor!;

    expect(
      thumbColor.resolve({WidgetState.selected, WidgetState.hovered}),
      appTheme.colorScheme.tertiaryContainer,
    );

    for (final stateCase in <({String name, Set<WidgetState> states})>[
      (name: 'empty', states: {}),
      (name: 'hovered only', states: {WidgetState.hovered}),
      (name: 'selected only', states: {WidgetState.selected}),
      (name: 'disabled', states: {WidgetState.disabled}),
      (
        name: 'disabled, selected, and hovered',
        states: {
          WidgetState.disabled,
          WidgetState.selected,
          WidgetState.hovered,
        },
      ),
    ]) {
      expect(
        thumbColor.resolve(stateCase.states),
        fallbackThumbColor,
        reason: stateCase.name,
      );
    }
  });
}
