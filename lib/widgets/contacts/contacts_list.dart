import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/widgets/common/index.dart';
import 'package:telepathy/widgets/contacts/contact_form.dart';
import 'package:telepathy/widgets/contacts/contact_widget.dart';
import 'package:telepathy/widgets/contacts/room_widget.dart';

/// A widget which displays a list of ContactWidgets.
class ContactsList extends StatelessWidget {
  final List<Contact> contacts;
  final List<Room> rooms;

  const ContactsList({super.key, required this.contacts, required this.rooms});

  static const _sectionStyle = TextStyle(
    fontSize: 12,
    color: Colors.white54,
    fontWeight: FontWeight.w600,
    letterSpacing: 0.6,
  );

  void _openAddDialog(BuildContext context) {
    showDialog(
      barrierDismissible: true,
      context: context,
      builder: (BuildContext dialogContext) {
        final scheme = Theme.of(dialogContext).colorScheme;
        final mq = MediaQuery.sizeOf(dialogContext);
        final dialogWidth = math.min(mq.width - 48, 420.0);
        final maxBodyHeight = math.max(200.0, mq.height * 0.85 - 72);

        return Dialog(
          insetPadding:
              const EdgeInsets.symmetric(horizontal: 24, vertical: 20),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
          ),
          backgroundColor: scheme.secondaryContainer,
          clipBehavior: Clip.antiAlias,
          child: SizedBox(
            width: dialogWidth,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
                  child: Text(
                    'New Contact or Room',
                    style: Theme.of(dialogContext).textTheme.titleLarge,
                  ),
                ),
                ConstrainedBox(
                  constraints: BoxConstraints(maxHeight: maxBodyHeight),
                  child: SingleChildScrollView(
                    padding: const EdgeInsets.fromLTRB(12, 4, 12, 16),
                    child: const ContactForm(),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    final bool showContactHeader =
        contacts.isNotEmpty && rooms.isNotEmpty;
    final bool showRoomHeader = showContactHeader;

    final physics = Theme.of(context).platform == TargetPlatform.iOS
        ? const BouncingScrollPhysics()
        : const ClampingScrollPhysics();

    return Container(
      padding: const EdgeInsets.only(bottom: 15, left: 12, right: 12, top: 8),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(10.0),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
              child: Row(
                children: [
                  const Padding(
                    padding: EdgeInsetsDirectional.only(bottom: 2),
                    child: Text('Contacts', style: TextStyle(fontSize: 20)),
                  ),
                  Padding(
                    padding: const EdgeInsets.only(left: 10, top: 3),
                    child: IconButton(
                        style: IconButton.styleFrom(
                          minimumSize: const Size(48, 48),
                        ),
                        onPressed: () => _openAddDialog(context),
                        padding: const EdgeInsetsDirectional.only(
                          start: 1,
                          top: 1,
                          end: 1,
                          bottom: 1,
                        ),
                        icon: SvgPicture.asset('assets/icons/Plus.svg')),
                  ),
                ],
              )),
          const SizedBox(height: 10.0),
          Flexible(
            fit: FlexFit.loose,
            child: Container(
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.tertiaryContainer,
                borderRadius: BorderRadius.circular(10),
              ),
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: contacts.isEmpty && rooms.isEmpty
                  ? Center(
                      child: Padding(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 24, vertical: 32),
                        child: Column(
                          mainAxisAlignment: MainAxisAlignment.center,
                          children: [
                            SvgPicture.asset(
                              'assets/icons/Group.svg',
                              width: 56,
                              height: 56,
                              semanticsLabel: 'No contacts',
                            ),
                            const SizedBox(height: 16),
                            Text(
                              'No contacts yet',
                              style: TextStyle(
                                fontSize: 16,
                                color: Theme.of(context)
                                    .colorScheme
                                    .onSecondaryContainer
                                    .withValues(alpha: 0.85),
                              ),
                            ),
                            const SizedBox(height: 20),
                            Button(
                              text: 'Add Contact',
                              onPressed: () => _openAddDialog(context),
                            ),
                          ],
                        ),
                      ),
                    )
                  : ListView(
                      physics: physics,
                      children: [
                        if (showContactHeader) ...[
                          const Padding(
                            padding: EdgeInsets.fromLTRB(14, 10, 14, 6),
                            child: Text('CONTACTS', style: _sectionStyle),
                          ),
                        ],
                        ...contacts.map(
                          (c) => ContactWidget(contact: c),
                        ),
                        if (showRoomHeader) ...[
                          const Padding(
                            padding: EdgeInsets.fromLTRB(14, 14, 14, 6),
                            child: Text('ROOMS', style: _sectionStyle),
                          ),
                        ],
                        ...rooms.map(
                          (r) => RoomWidget(room: r),
                        ),
                      ],
                    ),
            ),
          ),
        ],
      ),
    );
  }
}
