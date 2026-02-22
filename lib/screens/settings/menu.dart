import 'package:flutter/material.dart';
import 'package:telepathy/screens/settings/view.dart';

class SettingsMenu extends StatelessWidget {
  final SettingsSection selected;
  final void Function(SettingsSection) onSectionSelected;
  final bool showOverlayItem;

  const SettingsMenu({
    super.key,
    required this.selected,
    required this.onSectionSelected,
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
      onTap: () => onSectionSelected(section),
    );
  }
}

class SettingsMenuItem extends StatefulWidget {
  final String text;
  final bool selected;
  final VoidCallback onTap;

  const SettingsMenuItem({
    super.key,
    required this.text,
    required this.selected,
    required this.onTap,
  });

  @override
  State<SettingsMenuItem> createState() => _SettingsMenuItemState();
}

class _SettingsMenuItemState extends State<SettingsMenuItem> {
  bool _isHovered = false;

  @override
  Widget build(BuildContext context) {
    Color getColor() {
      if (_isHovered) {
        return Theme.of(context).colorScheme.secondary;
      } else if (widget.selected) {
        return Theme.of(context).colorScheme.primary;
      } else {
        return Theme.of(context).colorScheme.surfaceDim;
      }
    }

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 20),
      child: InkWell(
        mouseCursor: SystemMouseCursors.click,
        onTap: widget.onTap,
        onHover: (isHovered) {
          setState(() {
            _isHovered = isHovered;
          });
        },
        child: Container(
          padding: const EdgeInsets.symmetric(vertical: 5, horizontal: 10),
          width: 175,
          decoration: BoxDecoration(
            color: getColor(),
            borderRadius: BorderRadius.circular(5),
          ),
          child: Text(widget.text, style: const TextStyle(fontSize: 18)),
        ),
      ),
    );
  }
}
