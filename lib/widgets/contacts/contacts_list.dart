import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/widgets/contacts/contact_form.dart';
import 'package:telepathy/widgets/contacts/contact_widget.dart';
import 'package:telepathy/widgets/contacts/room_widget.dart';
import 'package:telepathy/widgets/contacts/snap_scroll_physics.dart';

/// A widget which displays a list of ContactWidgets.
class ContactsList extends StatelessWidget {
  final List<Contact> contacts;
  final List<Room> rooms;

  const ContactsList({super.key, required this.contacts, required this.rooms});

  @override
  Widget build(BuildContext context) {
    final stateController = context.watch<StateController>();
    final telepathy = context.read<Telepathy>();
    final ManagerState managerState = stateController.sessionManagerState;
    final bool isCompact = context.isCompactContacts || context.isCompactWide;
    final List<Object> items = [
      ...contacts,
      ...rooms,
    ];

    return Container(
      padding: EdgeInsets.only(
        bottom: isCompact ? 8 : 15,
        left: 12,
        right: 12,
        top: isCompact ? 2 : 8,
      ),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(10.0),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Padding(
              padding: EdgeInsets.symmetric(
                horizontal: 8,
                vertical: isCompact ? 0 : 7,
              ),
              child: Row(
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: [
                  Row(
                    children: [
                      const Padding(
                        padding: EdgeInsetsDirectional.only(bottom: 2),
                        child: Text('Contacts', style: TextStyle(fontSize: 20)),
                      ),
                      Padding(
                        padding: const EdgeInsets.only(left: 10, top: 3),
                        child: IconButton(
                            onPressed: () {
                              showDialog(
                                  context: context,
                                  builder: (BuildContext context) {
                                    return SimpleDialog(
                                      backgroundColor: Theme.of(context)
                                          .colorScheme
                                          .secondaryContainer,
                                      children: const [ContactForm()],
                                    );
                                  });
                            },
                            constraints: const BoxConstraints(
                              maxWidth: 36,
                              maxHeight: 36,
                            ),
                            padding: const EdgeInsetsDirectional.only(
                              start: 1,
                              top: 1,
                              end: 1,
                              bottom: 1,
                            ),
                            icon: SvgPicture.asset('assets/icons/Plus.svg')),
                      ),
                    ],
                  ),
                  Container(
                    decoration: BoxDecoration(
                      color: Theme.of(context).colorScheme.tertiaryContainer,
                      borderRadius: BorderRadius.circular(10),
                    ),
                    padding: const EdgeInsets.only(
                        left: 8, right: 3, top: 3, bottom: 3),
                    child: Tooltip(
                      message: switch (managerState) {
                        ManagerState.active => 'Session Manager Connected',
                        ManagerState.starting => 'Session Manager Starting…',
                        ManagerState.failed => 'Session Manager Failed',
                        ManagerState.stopped => 'Session Manager Inactive',
                      },
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          const Text('Session Manager'),
                          const SizedBox(width: 5),
                          switch (managerState) {
                            ManagerState.active => SvgPicture.asset(
                                'assets/icons/ShieldYes.svg',
                                semanticsLabel: 'Manager active icon',
                                colorFilter: ColorFilter.mode(
                                    Theme.of(context).colorScheme.primary,
                                    BlendMode.srcIn),
                                width: 28),
                            ManagerState.starting => const Padding(
                                padding: EdgeInsets.all(4),
                                child: SizedBox(
                                  width: 20,
                                  height: 20,
                                  child: CircularProgressIndicator(
                                    strokeWidth: 3,
                                  ),
                                ),
                              ),
                            ManagerState.failed ||
                            ManagerState.stopped =>
                              SvgPicture.asset('assets/icons/ShieldOff.svg',
                                  colorFilter: const ColorFilter.mode(
                                    Color(0xFFdc2626),
                                    BlendMode.srcIn,
                                  ),
                                  semanticsLabel: 'Manager inactive icon',
                                  width: 28),
                          },
                          if (managerState == ManagerState.failed) ...[
                            const SizedBox(width: 10),
                            IconButton(
                                onPressed: () {
                                  telepathy.restartManager();
                                },
                                constraints: const BoxConstraints(
                                  maxWidth: 36,
                                  maxHeight: 36,
                                ),
                                padding: const EdgeInsetsDirectional.only(
                                  start: 1,
                                  top: 1,
                                  end: 1,
                                  bottom: 1,
                                ),
                                icon: SvgPicture.asset(
                                    'assets/icons/Restart.svg',
                                    colorFilter: const ColorFilter.mode(
                                        Color(0xFFdc2626), BlendMode.srcIn),
                                    semanticsLabel: 'Restart session manager')),
                          ],
                        ],
                      ),
                    ),
                  )
                ],
              )),
          SizedBox(height: isCompact ? 2.5 : 10),
          Flexible(
            fit: FlexFit.loose,
            child: Container(
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.tertiaryContainer,
                borderRadius: BorderRadius.circular(10),
              ),
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: LayoutBuilder(builder: (context, constraints) {
                final itemHeight = constraints.maxHeight / (isCompact ? 2 : 3);

                return ListView.builder(
                  itemCount: items.length,
                  itemExtent: itemHeight, // every item = 1/3 of viewport
                  physics: SnapScrollPhysics(itemExtent: itemHeight),
                  itemBuilder: (BuildContext context, int index) {
                    final item = items[index];

                    if (item is Contact) {
                      return ContactWidget(
                        contact: item,
                      );
                    } else if (item is Room) {
                      return RoomWidget(
                        room: item,
                      );
                    } else {
                      return const SizedBox.shrink();
                    }
                  },
                );
              }),
            ),
          ),
        ],
      ),
    );
  }
}
