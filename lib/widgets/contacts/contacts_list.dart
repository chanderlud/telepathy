import 'package:flutter/foundation.dart';
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

const double managerStatusSize = 28;
const double contactsHeaderHeight = 36;
const double addButtonSize = 36;
const double addIconSize = 28;

/// A widget which displays a list of ContactWidgets.
class ContactsList extends StatelessWidget {
  final List<Contact> contacts;
  final List<Room> rooms;

  const ContactsList({super.key, required this.contacts, required this.rooms});

  @override
  Widget build(BuildContext context) {
    final bool isWindowsDesktop =
        !kIsWeb && defaultTargetPlatform == TargetPlatform.windows;
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
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          Padding(
              padding: EdgeInsets.symmetric(
                horizontal: 8,
                vertical: isCompact ? 0 : 7,
              ),
              child: Row(
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: [
                  SizedBox(
                    height: contactsHeaderHeight,
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.center,
                      children: [
                        const Text(
                          'Contacts',
                          style: TextStyle(
                            fontSize: 20,
                            height: 1.0,
                            leadingDistribution: TextLeadingDistribution.even,
                          ),
                          strutStyle: StrutStyle(
                            fontSize: 20,
                            height: 1.0,
                            leading: 0,
                            forceStrutHeight: true,
                          ),
                          textHeightBehavior: TextHeightBehavior(
                            applyHeightToFirstAscent: false,
                            applyHeightToLastDescent: false,
                            leadingDistribution: TextLeadingDistribution.even,
                          ),
                        ),
                        const SizedBox(width: 10),
                        SizedBox.square(
                          dimension: addButtonSize,
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
                                },
                              );
                            },
                            constraints: const BoxConstraints.tightFor(
                              width: addButtonSize,
                              height: addButtonSize,
                            ),
                            padding: isWindowsDesktop
                                ? const EdgeInsets.only(top: 2.0)
                                : EdgeInsets.zero,
                            icon: SvgPicture.asset(
                              'assets/icons/Plus.svg',
                              width: addIconSize,
                              height: addIconSize,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                  Transform.translate(
                    offset: Offset(0, isCompact ? 0 : 1.5),
                    child: Container(
                      height: 34,
                      decoration: BoxDecoration(
                        color: Theme.of(context).colorScheme.tertiaryContainer,
                        borderRadius: BorderRadius.circular(10),
                      ),
                      padding: const EdgeInsets.only(left: 8, right: 3),
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.center,
                        children: [
                          const Text(
                            'Session Manager',
                            style: TextStyle(
                              height: 1.0,
                              leadingDistribution: TextLeadingDistribution.even,
                            ),
                            strutStyle: StrutStyle(
                              height: 1.0,
                              leading: 0,
                              forceStrutHeight: true,
                            ),
                            textHeightBehavior: TextHeightBehavior(
                              applyHeightToFirstAscent: false,
                              applyHeightToLastDescent: false,
                              leadingDistribution: TextLeadingDistribution.even,
                            ),
                          ),
                          const SizedBox(width: 5),
                          managerStatusIcon(context, managerState),
                          if (managerState == ManagerState.failed) ...[
                            const SizedBox(width: 10),
                            SizedBox.square(
                              dimension: 28,
                              child: IconButton(
                                onPressed: () {
                                  telepathy.restartManager();
                                },
                                constraints: const BoxConstraints.tightFor(
                                  width: 28,
                                  height: 28,
                                ),
                                padding: EdgeInsets.zero,
                                icon: SvgPicture.asset(
                                  'assets/icons/Restart.svg',
                                  colorFilter: const ColorFilter.mode(
                                    Color(0xFFdc2626),
                                    BlendMode.srcIn,
                                  ),
                                  semanticsLabel: 'Restart session manager',
                                  width: 24,
                                  height: 24,
                                ),
                              ),
                            ),
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

Widget managerStatusIcon(BuildContext context, ManagerState managerState) {
  final colorScheme = Theme.of(context).colorScheme;

  return SizedBox.square(
    dimension: managerStatusSize,
    child: Center(
      child: switch (managerState) {
        ManagerState.active => SvgPicture.asset(
            'assets/icons/ShieldYes.svg',
            semanticsLabel: 'Manager active icon',
            colorFilter: ColorFilter.mode(
              colorScheme.primary,
              BlendMode.srcIn,
            ),
            width: managerStatusSize,
            height: managerStatusSize,
          ),
        ManagerState.starting => const SizedBox.square(
            dimension: 18,
            child: CircularProgressIndicator(
              strokeWidth: 3,
            ),
          ),
        ManagerState.failed || ManagerState.stopped => SvgPicture.asset(
            'assets/icons/ShieldOff.svg',
            semanticsLabel: 'Manager inactive icon',
            colorFilter: const ColorFilter.mode(
              Color(0xFFdc2626),
              BlendMode.srcIn,
            ),
            width: managerStatusSize,
            height: managerStatusSize,
          ),
      },
    ),
  );
}
