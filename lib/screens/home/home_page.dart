import 'package:flutter/material.dart' hide Overlay;
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/call/call_controls.dart';
import 'package:telepathy/widgets/call/call_details_widget.dart';
import 'package:telepathy/widgets/call/room_details_widget.dart';
import 'package:telepathy/widgets/chat/chat_widget.dart';
import 'package:telepathy/widgets/contacts/contacts_list.dart';
import 'package:telepathy/widgets/home/home_tab_view.dart';
import 'package:telepathy/src/rust/flutter.dart';

/// The main body of the app.
class HomePage extends StatelessWidget {
  final Telepathy telepathy;
  final SettingsController settingsController;
  final InterfaceController interfaceController;
  final StateController stateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final ChatStateController chatStateController;
  final Overlay overlay;
  final AudioDevices audioDevices;

  const HomePage(
      {super.key,
      required this.telepathy,
      required this.settingsController,
      required this.interfaceController,
      required this.stateController,
      required this.player,
      required this.chatStateController,
      required this.statisticsController,
      required this.overlay,
      required this.audioDevices});

  @override
  Widget build(BuildContext context) {
    PeriodicNotifier notifier = PeriodicNotifier();

    CallControls callControls = CallControls(
      telepathy: telepathy,
      settingsController: settingsController,
      interfaceController: interfaceController,
      stateController: stateController,
      statisticsController: statisticsController,
      player: player,
      notifier: notifier,
      overlay: overlay,
      audioDevices: audioDevices,
    );

    ChatWidget chatWidget = ChatWidget(
        telepathy: telepathy,
        stateController: stateController,
        chatStateController: chatStateController,
        player: player,
        settingsController: settingsController);

    return Scaffold(
      body: Padding(
          padding: const EdgeInsets.all(20.0),
          child: SafeArea(
              bottom: false,
              child: LayoutBuilder(
                  builder: (BuildContext context, BoxConstraints constraints) {
                ListenableBuilder contactsList = ListenableBuilder(
                    listenable: settingsController,
                    builder: (BuildContext context, Widget? child) {
                      return ListenableBuilder(
                          listenable: stateController,
                          builder: (BuildContext context, Widget? child) {
                            List<Contact> contacts =
                                settingsController.contacts.values.toList();

                            // sort contacts by session status then nickname
                            contacts.sort((a, b) {
                              SessionStatus aStatus =
                                  stateController.sessionStatus(a);
                              SessionStatus bStatus =
                                  stateController.sessionStatus(b);

                              if (aStatus == bStatus) {
                                return a.nickname().compareTo(b.nickname());
                              } else if (aStatus.runtimeType ==
                                  SessionStatus_Connected) {
                                return -1;
                              } else if (bStatus.runtimeType ==
                                  SessionStatus_Connected) {
                                return 1;
                              } else if (aStatus.runtimeType ==
                                  SessionStatus_Connecting) {
                                return -1;
                              } else if (bStatus.runtimeType ==
                                  SessionStatus_Connecting) {
                                return 1;
                              } else {
                                return 0;
                              }
                            });

                            return ContactsList(
                              telepathy: telepathy,
                              contacts: contacts,
                              rooms: settingsController.rooms.values.toList(),
                              stateController: stateController,
                              settingsController: settingsController,
                              player: player,
                            );
                          });
                    });

                if (constraints.maxWidth > 600) {
                  return Column(
                    children: [
                      ListenableBuilder(
                        listenable: stateController,
                        builder: (BuildContext context, Widget? child) {
                          return Container(
                              constraints: const BoxConstraints(maxHeight: 275),
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  // Animated call-details / form area
                                  AnimatedSize(
                                    duration: const Duration(milliseconds: 250),
                                    curve: Curves.easeInOut,
                                    alignment: Alignment.centerLeft,
                                    child: stateController.isCallActive
                                        ? Row(
                                            mainAxisSize: MainAxisSize.min,
                                            children: [
                                              Container(
                                                constraints:
                                                    const BoxConstraints(
                                                        maxWidth: 300),
                                                child: CallDetailsWidget(
                                                    statisticsController:
                                                        statisticsController,
                                                    stateController:
                                                        stateController),
                                              ),
                                              const SizedBox(width: 20),
                                            ],
                                          )
                                        : const SizedBox.shrink(),
                                  ),

                                  // Contacts list always present, just expands when the left bit collapses
                                  Flexible(
                                    fit: FlexFit.loose,
                                    child: stateController.activeRoom != null
                                        ? RoomDetailsWidget(
                                            telepathy: telepathy,
                                            stateController: stateController,
                                            player: player,
                                            settingsController:
                                                settingsController,
                                          )
                                        : contactsList,
                                  ),
                                ],
                              ));
                        },
                      ),
                      const SizedBox(height: 20),
                      Flexible(
                          fit: FlexFit.loose,
                          child: Row(mainAxisSize: MainAxisSize.min, children: [
                            Container(
                                constraints:
                                    const BoxConstraints(maxWidth: 260),
                                decoration: BoxDecoration(
                                  color: Theme.of(context)
                                      .colorScheme
                                      .tertiaryContainer,
                                  borderRadius: BorderRadius.circular(10.0),
                                ),
                                child: callControls),
                            const SizedBox(width: 20),
                            Flexible(
                                fit: FlexFit.loose,
                                child: Container(
                                    decoration: BoxDecoration(
                                      color: Theme.of(context)
                                          .colorScheme
                                          .secondaryContainer,
                                      borderRadius: BorderRadius.circular(10.0),
                                    ),
                                    padding: const EdgeInsets.only(
                                        left: 10,
                                        right: 10,
                                        top: 5,
                                        bottom: 10),
                                    child: chatWidget))
                          ])),
                    ],
                  );
                } else {
                  return Column(children: [
                    Container(
                      constraints: BoxConstraints(
                          maxHeight: 250, maxWidth: constraints.maxWidth),
                      child: contactsList,
                    ),
                    const SizedBox(height: 20),
                    HomeTabView(
                        widgetOne: callControls,
                        widgetTwo: Padding(
                            padding: const EdgeInsets.only(
                                left: 10, right: 10, top: 5, bottom: 10),
                            child: chatWidget),
                        colorOne:
                            Theme.of(context).colorScheme.tertiaryContainer,
                        colorTwo:
                            Theme.of(context).colorScheme.secondaryContainer,
                        iconOne: const Icon(Icons.call),
                        iconTwo: const Icon(Icons.chat))
                  ]);
                }
              }))),
    );
  }
}
