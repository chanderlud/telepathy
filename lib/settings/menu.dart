import 'package:flutter/material.dart';
import 'package:telepathy/settings/view.dart';

class SettingsMenu extends StatelessWidget {
  final SettingsSection selected;
  final SettingsSection? hovered;
  final void Function(SettingsSection) onSectionSelected;
  final void Function(SettingsSection, bool) onHoverChanged;
  final bool showOverlayItem;

  const SettingsMenu({
    super.key,
    required this.selected,
    required this.hovered,
    required this.onSectionSelected,
    required this.onHoverChanged,
    required this.showOverlayItem,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisAlignment: MainAxisAlignment.start,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _buildItem(context, SettingsSection.audioVideo, 'Audio & Video'),
        const SizedBox(height: 12),
        _buildItem(context, SettingsSection.profiles, 'Profiles'),
        const SizedBox(height: 12),
        _buildItem(context, SettingsSection.networking, 'Networking'),
        const SizedBox(height: 12),
        _buildItem(context, SettingsSection.interface, 'Interface'),
        const SizedBox(height: 12),
        _buildItem(context, SettingsSection.logs, 'View Log'),
        if (showOverlayItem) ...[
          const SizedBox(height: 12),
          _buildItem(context, SettingsSection.overlay, 'Overlay'),
        ],
      ],
    );
  }

  Widget _buildItem(
    BuildContext context,
    SettingsSection section,
    String text,
  ) {
    return SettingsMenuItem(
      text: text,
      selected: selected == section,
      hovered: hovered == section,
      onTap: () => onSectionSelected(section),
      onEnter: () => onHoverChanged(section, true),
      onExit: () => onHoverChanged(section, false),
    );
  }
}

class SettingsMenuItem extends StatelessWidget {
  final String text;
  final bool selected;
  final bool hovered;
  final VoidCallback onTap;
  final VoidCallback onEnter;
  final VoidCallback onExit;

  const SettingsMenuItem({
    super.key,
    required this.text,
    required this.selected,
    required this.hovered,
    required this.onTap,
    required this.onEnter,
    required this.onExit,
  });

  @override
  Widget build(BuildContext context) {
    Color getColor() {
      if (hovered) {
        return Theme.of(context).colorScheme.secondary;
      } else if (selected) {
        return Theme.of(context).colorScheme.primary;
      } else {
        return Theme.of(context).colorScheme.surfaceDim;
      }
    }

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 20),
      child: InkWell(
        onTap: onTap,
        onHover: (isHovered) => isHovered ? onEnter() : onExit(),
        child: Container(
          padding: const EdgeInsets.symmetric(vertical: 5, horizontal: 10),
          width: 175,
          decoration: BoxDecoration(
            color: getColor(),
            borderRadius: BorderRadius.circular(5),
          ),
          child: Text(text, style: const TextStyle(fontSize: 18)),
        ),
      ),
    );
  }
}
