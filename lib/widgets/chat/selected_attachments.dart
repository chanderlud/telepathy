import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

class SelectedAttachments extends StatefulWidget {
  final List<(String, Uint8List)> attachments;
  final void Function(String name) onRemove;

  const SelectedAttachments({
    super.key,
    required this.attachments,
    required this.onRemove,
  });

  @override
  State<SelectedAttachments> createState() => _SelectedAttachmentsState();
}

class _SelectedAttachmentsState extends State<SelectedAttachments> {
  final Map<String, bool> _hovered = {};

  @override
  Widget build(BuildContext context) {
    final chips = widget.attachments.map((attachment) {
      final name = attachment.$1;

      return InkWell(
        mouseCursor: SystemMouseCursors.basic,
        onTap: () {},
        onHover: (hovered) => setState(() => _hovered[name] = hovered),
        child: Stack(
          children: [
            Container(
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.tertiaryContainer,
                borderRadius: BorderRadius.circular(10.0),
                border: Border.all(color: Colors.grey.shade400),
              ),
              margin: const EdgeInsets.only(top: 5, right: 5),
              child: Padding(
                padding:
                    const EdgeInsets.only(left: 4, right: 4, top: 2, bottom: 4),
                child: Text(name),
              ),
            ),
            if (_hovered[name] ?? false)
              Positioned(
                right: 0,
                child: InkWell(
                  onTap: () => widget.onRemove(name),
                  child: Container(
                    decoration: BoxDecoration(
                      color: Theme.of(context).colorScheme.tertiaryContainer,
                      borderRadius: BorderRadius.circular(10.0),
                    ),
                    child: SvgPicture.asset(
                      'assets/icons/Trash.svg',
                      semanticsLabel: 'Close attachment icon',
                      colorFilter: const ColorFilter.mode(
                        Color(0xFFdc2626),
                        BlendMode.srcIn,
                      ),
                      width: 20,
                    ),
                  ),
                ),
              ),
          ],
        ),
      );
    }).toList();

    return Wrap(children: chips);
  }
}
