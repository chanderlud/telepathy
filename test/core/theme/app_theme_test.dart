import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/core/theme/app_theme.dart';

void main() {
  testWidgets('uses bundled Nunito typography roles for app hierarchy', (
    WidgetTester tester,
  ) async {
    late ThemeData appTheme;

    await tester.pumpWidget(
      MaterialApp(
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

    final roles = <TextStyle?>[
      appTheme.textTheme.titleLarge,
      appTheme.textTheme.titleMedium,
      appTheme.textTheme.labelLarge,
      appTheme.textTheme.bodyMedium,
      appTheme.textTheme.bodySmall,
    ];

    for (final role in roles) {
      expect(role?.fontFamily, 'Nunito');
    }
    expect(appTheme.textTheme.titleLarge?.fontSize, 20);
    expect(appTheme.textTheme.titleLarge?.fontWeight, FontWeight.w700);
    expect(appTheme.textTheme.titleMedium?.fontSize, 16);
    expect(appTheme.textTheme.titleMedium?.fontWeight, FontWeight.w600);
    expect(appTheme.textTheme.labelLarge?.fontSize, 14);
    expect(appTheme.textTheme.labelLarge?.fontWeight, FontWeight.w600);
    expect(appTheme.textTheme.bodyMedium?.fontSize, 14);
    expect(appTheme.textTheme.bodyMedium?.fontWeight, FontWeight.w400);
    expect(appTheme.textTheme.bodySmall?.fontSize, 12);
    expect(appTheme.textTheme.bodySmall?.fontWeight, FontWeight.w400);
  });

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
      appTheme.colorScheme.surfaceDim,
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
