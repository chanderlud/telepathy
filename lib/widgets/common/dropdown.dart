import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

class DropDown<T> extends StatelessWidget {
  final String? label;
  final List<(String, String)> items;
  final String? initialSelection;
  final void Function(String?) onSelected;
  final double? width;
  final bool enabled;

  const DropDown(
      {super.key,
      this.label,
      required this.items,
      required this.initialSelection,
      required this.onSelected,
      this.width,
      this.enabled = true});

  @override
  Widget build(BuildContext context) {
    return DropdownMenu<String>(
      width: width,
      label: label == null ? null : Text(label!),
      enabled: enabled,
      dropdownMenuEntries: items.map((item) {
        return DropdownMenuEntry(
          value: item.$1,
          label: item.$2,
        );
      }).toList(),
      onSelected: onSelected,
      initialSelection: initialSelection,
      trailingIcon: SvgPicture.asset(
        'assets/icons/DropdownDown.svg',
        semanticsLabel: 'Open Dropdown',
        width: 20,
      ),
      selectedTrailingIcon: SvgPicture.asset(
        'assets/icons/DropdownUp.svg',
        semanticsLabel: 'Close Dropdown',
        width: 20,
      ),
    );
  }
}
