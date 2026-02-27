import 'package:flutter/material.dart';
import 'package:flutter_svg/svg.dart';

class SettingsHeader extends StatelessWidget {
  final bool isNarrow;
  final bool showMenu;
  final VoidCallback onBack;
  final VoidCallback onToggleMenu;

  const SettingsHeader({
    super.key,
    required this.isNarrow,
    required this.showMenu,
    required this.onBack,
    required this.onToggleMenu,
  });

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: Alignment.topLeft,
      child: Container(
        padding: const EdgeInsets.only(left: 5, top: 5, bottom: 5),
        decoration: BoxDecoration(
          color:
              showMenu ? null : Theme.of(context).colorScheme.tertiaryContainer,
          borderRadius: const BorderRadius.only(
            bottomLeft: Radius.circular(8),
            bottomRight: Radius.circular(8),
          ),
        ),
        child: Row(
          children: [
            IconButton(
              visualDensity: VisualDensity.comfortable,
              icon: SvgPicture.asset(
                'assets/icons/Back.svg',
                semanticsLabel: 'Close Settings',
                width: 30,
              ),
              onPressed: onBack,
            ),
            const SizedBox(width: 3),
            if (isNarrow)
              IconButton(
                visualDensity: VisualDensity.comfortable,
                icon: SvgPicture.asset(
                  showMenu
                      ? 'assets/icons/HamburgerOpened.svg'
                      : 'assets/icons/HamburgerClosed.svg',
                  semanticsLabel: 'Menu',
                  width: 30,
                ),
                onPressed: onToggleMenu,
              ),
          ],
        ),
      ),
    );
  }
}
