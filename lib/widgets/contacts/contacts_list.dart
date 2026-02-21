import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/widgets/contacts/contact_form.dart';
import 'package:telepathy/widgets/contacts/contact_widget.dart';
import 'package:telepathy/widgets/contacts/room_widget.dart';
import 'package:telepathy/widgets/contacts/snap_scroll_physics.dart';

/// A widget which displays a list of ContactWidgets.
class ContactsList extends StatelessWidget {
  final List<Contact> contacts;
  final List<Room> rooms;

  const ContactsList(
      {super.key,
      required this.contacts,
      required this.rooms});

  @override
  Widget build(BuildContext context) {
    final List<Object> items = [
      ...contacts,
      ...rooms,
    ];

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
                        onPressed: () {
                          showDialog(
                              context: context,
                              builder: (BuildContext context) {
                                return SimpleDialog(
                                  backgroundColor: Theme.of(context)
                                      .colorScheme
                                      .secondaryContainer,
                                  children: const [
                                    ContactForm()
                                  ],
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
              child: LayoutBuilder(builder: (context, constraints) {
                final itemHeight = constraints.maxHeight / 3;

                return ListView.builder(
                  itemCount: items.length,
                  itemExtent: itemHeight, // every item = 1/3 of viewport
                  physics: SnapScrollPhysics(itemExtent: itemHeight),
                  itemBuilder: (BuildContext context, int index) {
                    return Consumer<StateController>(
                      builder: (BuildContext context,
                          StateController stateController, _) {
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
