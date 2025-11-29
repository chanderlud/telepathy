import 'dart:core';
import 'package:flutter/services.dart';
import 'package:telepathy/settings/controller.dart';
import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/main.dart';
import 'package:telepathy/src/rust/telepathy.dart';

class ProfileSettings extends StatefulWidget {
  final SettingsController controller;
  final Telepathy telepathy;
  final StateController stateController;

  const ProfileSettings(
      {super.key,
        required this.controller,
        required this.telepathy,
        required this.stateController});

  @override
  ProfileSettingsState createState() => ProfileSettingsState();
}

class ProfileSettingsState extends State<ProfileSettings> {
  final TextEditingController _profileNameInput = TextEditingController();

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(5),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.end,
        mainAxisSize: MainAxisSize.min,
        children: [
          ListenableBuilder(
              listenable: widget.controller,
              builder: (BuildContext context, Widget? child) {
                bool even = widget.controller.profiles.length % 2 == 0;

                Color colorPicker(int index) {
                  if (even ? index % 2 == 0 : index % 2 != 0) {
                    return Colors.transparent;
                  } else {
                    return Theme.of(context).colorScheme.secondaryContainer;
                  }
                }

                return ListView.builder(
                    shrinkWrap: true,
                    itemCount: widget.controller.profiles.length,
                    itemBuilder: (BuildContext context, int index) {
                      Profile profile =
                      widget.controller.profiles.values.elementAt(index);

                      return Container(
                        decoration: BoxDecoration(
                          color: colorPicker(index),
                          borderRadius: index == 0
                              ? const BorderRadius.only(
                              topLeft: Radius.circular(5),
                              topRight: Radius.circular(5))
                              : null,
                        ),
                        padding: const EdgeInsets.only(
                            top: 5, bottom: 5, left: 20, right: 10),
                        child: Row(
                          children: [
                            Text(
                              profile.nickname,
                              style: const TextStyle(fontSize: 18),
                            ),
                            const Spacer(),
                            ListenableBuilder(
                                listenable: widget.stateController,
                                builder: (BuildContext context, Widget? child) {
                                  return Button(
                                    text: (widget.controller.activeProfile ==
                                        profile.id)
                                        ? 'Active'
                                        : 'Set Active',
                                    width: 65,
                                    height: 25,
                                    disabled:
                                    widget.stateController.isCallActive ||
                                        widget.controller.activeProfile ==
                                            profile.id,
                                    onPressed: () {
                                      widget.controller
                                          .setActiveProfile(profile.id);
                                      widget.telepathy
                                          .setIdentity(key: profile.keypair);
                                      widget.telepathy.restartManager();
                                    },
                                    noSplash: true,
                                    disabledColor: widget
                                        .controller.activeProfile ==
                                        profile.id &&
                                        widget.stateController.isCallActive
                                        ? Theme.of(context)
                                        .colorScheme
                                        .tertiaryContainer
                                        : null,
                                  );
                                }),
                            const SizedBox(width: 10),
                            IconButton(
                                tooltip: 'Copy Peer ID',
                                onPressed: () {
                                  Clipboard.setData(
                                      ClipboardData(text: profile.peerId));
                                },
                                icon: SvgPicture.asset(
                                  'assets/icons/Copy.svg',
                                  semanticsLabel: 'Copy Peer ID',
                                  width: 26,
                                )),
                            IconButton(
                              tooltip: 'Delete Profile',
                              onPressed: () {
                                showDialog(
                                    context: context,
                                    builder: (BuildContext context) {
                                      return AlertDialog(
                                        title: const Text('Delete Profile'),
                                        content: const Text(
                                            'Are you sure you want to delete this profile?'),
                                        actions: [
                                          Button(
                                            text: 'Cancel',
                                            onPressed: () {
                                              Navigator.of(context).pop();
                                            },
                                          ),
                                          Button(
                                            text: 'Delete',
                                            onPressed: () {
                                              widget.controller
                                                  .removeProfile(profile.id);
                                              Navigator.of(context).pop();
                                            },
                                          )
                                        ],
                                      );
                                    });
                              },
                              icon: SvgPicture.asset(
                                'assets/icons/Trash.svg',
                                semanticsLabel: 'Delete Profile',
                                width: 26,
                              ),
                            ),
                          ],
                        ),
                      );
                    });
              }),
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 5, horizontal: 20),
            child: IconButton(
              onPressed: () {
                showDialog(
                    context: context,
                    builder: (BuildContext context) {
                      return SimpleDialog(
                        title: const Text('Create Profile'),
                        contentPadding: const EdgeInsets.only(
                            bottom: 25, left: 25, right: 25),
                        titlePadding: const EdgeInsets.only(
                            top: 25, left: 25, right: 25, bottom: 15),
                        children: [
                          TextField(
                            decoration: const InputDecoration(
                              labelText: 'Name',
                            ),
                            controller: _profileNameInput,
                          ),
                          const SizedBox(height: 20),
                          Button(
                            text: 'Create',
                            onPressed: () {
                              widget.controller
                                  .createProfile(_profileNameInput.text);
                              _profileNameInput.clear();
                              Navigator.of(context).pop();
                            },
                          )
                        ],
                      );
                    });
              },
              visualDensity: VisualDensity.comfortable,
              icon: SvgPicture.asset(
                'assets/icons/Plus.svg',
                semanticsLabel: 'Create Profile',
                width: 38,
              ),
              tooltip: 'Create Profile',
            ),
          ),
        ],
      ),
    );
  }
}