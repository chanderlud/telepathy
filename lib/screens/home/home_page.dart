import 'package:flutter/material.dart' hide Overlay;
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/call/call.dart';
import 'package:telepathy/widgets/chat/chat.dart';
import 'package:telepathy/widgets/contacts/contacts.dart';
import 'package:telepathy/widgets/home/home_tab_view.dart';

/// The main body of the app.
class HomePage extends StatefulWidget {
  final Telepathy telepathy;
  final ProfilesController profilesController;
  final AudioSettingsController audioSettingsController;
  final NetworkSettingsController networkSettingsController;
  final PreferencesController preferencesController;
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
      required this.profilesController,
      required this.audioSettingsController,
      required this.networkSettingsController,
      required this.preferencesController,
      required this.interfaceController,
      required this.stateController,
      required this.player,
      required this.chatStateController,
      required this.statisticsController,
      required this.overlay,
      required this.audioDevices});

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  late final PeriodicNotifier notifier;
  late CallControls callControls;
  late ChatWidget chatWidget;

  CallControls _createCallControls() {
    return CallControls(
      telepathy: widget.telepathy,
      profilesController: widget.profilesController,
      audioSettingsController: widget.audioSettingsController,
      networkSettingsController: widget.networkSettingsController,
      preferencesController: widget.preferencesController,
      interfaceController: widget.interfaceController,
      stateController: widget.stateController,
      statisticsController: widget.statisticsController,
      player: widget.player,
      notifier: notifier,
      overlay: widget.overlay,
      audioDevices: widget.audioDevices,
    );
  }

  ChatWidget _createChatWidget() {
    return ChatWidget(
        telepathy: widget.telepathy,
        stateController: widget.stateController,
        chatStateController: widget.chatStateController,
        player: widget.player,
        profilesController: widget.profilesController);
  }

  @override
  void initState() {
    super.initState();
    notifier = PeriodicNotifier();

    callControls = _createCallControls();
    chatWidget = _createChatWidget();
  }

  @override
  void didUpdateWidget(covariant HomePage oldWidget) {
    super.didUpdateWidget(oldWidget);

    final bool callControlsDepsChanged = widget.telepathy !=
            oldWidget.telepathy ||
        widget.profilesController != oldWidget.profilesController ||
        widget.audioSettingsController != oldWidget.audioSettingsController ||
        widget.networkSettingsController !=
            oldWidget.networkSettingsController ||
        widget.preferencesController != oldWidget.preferencesController ||
        widget.interfaceController != oldWidget.interfaceController ||
        widget.stateController != oldWidget.stateController ||
        widget.statisticsController != oldWidget.statisticsController ||
        widget.player != oldWidget.player ||
        widget.overlay != oldWidget.overlay ||
        widget.audioDevices != oldWidget.audioDevices;

    final bool chatWidgetDepsChanged =
        widget.telepathy != oldWidget.telepathy ||
            widget.stateController != oldWidget.stateController ||
            widget.chatStateController != oldWidget.chatStateController ||
            widget.player != oldWidget.player ||
            widget.profilesController != oldWidget.profilesController;

    if (callControlsDepsChanged || chatWidgetDepsChanged) {
      setState(() {
        if (callControlsDepsChanged) {
          callControls = _createCallControls();
        }
        if (chatWidgetDepsChanged) {
          chatWidget = _createChatWidget();
        }
      });
    }
  }

  @override
  void dispose() {
    notifier.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Padding(
          padding: const EdgeInsets.all(20.0),
          child: SafeArea(
              bottom: false,
              child: LayoutBuilder(
                  builder: (BuildContext context, BoxConstraints constraints) {
                final Widget contactsList = SortedContactsList(
                  telepathy: widget.telepathy,
                  profilesController: widget.profilesController,
                  stateController: widget.stateController,
                  player: widget.player,
                );

                if (constraints.maxWidth > 600) {
                  return Column(
                    children: [
                      ListenableBuilder(
                        listenable: widget.stateController,
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
                                    child: widget.stateController.isCallActive
                                        ? Row(
                                            mainAxisSize: MainAxisSize.min,
                                            children: [
                                              Container(
                                                constraints:
                                                    const BoxConstraints(
                                                        maxWidth: 300),
                                                child: CallDetailsWidget(
                                                    statisticsController: widget
                                                        .statisticsController,
                                                    stateController:
                                                        widget.stateController),
                                              ),
                                              const SizedBox(width: 20),
                                            ],
                                          )
                                        : const SizedBox.shrink(),
                                  ),

                                  // Contacts list always present, just expands when the left bit collapses
                                  Flexible(
                                    fit: FlexFit.loose,
                                    child: widget.stateController.activeRoom !=
                                            null
                                        ? RoomDetailsWidget(
                                            telepathy: widget.telepathy,
                                            stateController:
                                                widget.stateController,
                                            player: widget.player,
                                            profilesController:
                                                widget.profilesController,
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
