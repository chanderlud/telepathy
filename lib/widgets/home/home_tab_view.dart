import 'package:flutter/material.dart';

/// A tab view used to display call controls, chat, and optionally call details.
class HomeTabView extends StatefulWidget {
  final Widget widgetOne;
  final Widget widgetTwo;
  final Color colorOne;
  final Color colorTwo;
  final Icon iconOne;
  final Icon iconTwo;
  final Widget? widgetThree;
  final Color? colorThree;
  final Icon? iconThree;

  const HomeTabView(
      {super.key,
      required this.widgetOne,
      required this.widgetTwo,
      required this.colorOne,
      required this.colorTwo,
      required this.iconOne,
      required this.iconTwo,
      this.widgetThree,
      this.colorThree,
      this.iconThree});

  @override
  State<HomeTabView> createState() => HomeTabViewState();
}

class HomeTabViewState extends State<HomeTabView>
    with SingleTickerProviderStateMixin {
  late TabController _tabController;
  late Color _backgroundColor = widget.colorOne;

  int get _tabLength => widget.widgetThree != null ? 3 : 2;

  void _syncBackground(int index) {
    if (index == 0) {
      _backgroundColor = widget.colorOne;
    } else if (index == 1) {
      _backgroundColor = widget.colorTwo;
    } else {
      _backgroundColor = widget.colorThree ?? widget.colorTwo;
    }
  }

  @override
  void initState() {
    super.initState();
    _tabController = TabController(length: _tabLength, vsync: this);
    _tabController.addListener(() {
      setState(() {
        _syncBackground(_tabController.index);
      });
    });
  }

  @override
  void didUpdateWidget(HomeTabView oldWidget) {
    super.didUpdateWidget(oldWidget);
    final oldLen = oldWidget.widgetThree != null ? 3 : 2;
    final newLen = widget.widgetThree != null ? 3 : 2;
    if (oldLen != newLen) {
      _tabController.dispose();
      _tabController = TabController(length: newLen, vsync: this);
      _tabController.addListener(() {
        setState(() {
          _syncBackground(_tabController.index);
        });
      });
      _syncBackground(_tabController.index.clamp(0, newLen - 1));
    }
  }

  @override
  void dispose() {
    _tabController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Flexible(
      fit: FlexFit.loose,
      child: Container(
        decoration: BoxDecoration(
          color: _backgroundColor,
          borderRadius: BorderRadius.circular(10.0),
        ),
        child: Column(
          children: [
            Container(
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.secondaryContainer,
                borderRadius:
                    const BorderRadius.vertical(top: Radius.circular(10.0)),
              ),
              padding: const EdgeInsets.symmetric(vertical: 12),
              child: TabBar(
                controller: _tabController,
                splashFactory: NoSplash.splashFactory,
                overlayColor: WidgetStateProperty.all(Colors.transparent),
                dividerHeight: 0,
                padding: const EdgeInsets.all(0),
                tabs: [
                  Tab(icon: widget.iconOne),
                  Tab(icon: widget.iconTwo),
                  if (widget.widgetThree != null && widget.iconThree != null)
                    Tab(icon: widget.iconThree),
                ],
              ),
            ),
            Flexible(
              child: TabBarView(
                controller: _tabController,
                children: [
                  widget.widgetOne,
                  widget.widgetTwo,
                  if (widget.widgetThree != null) widget.widgetThree!,
                ],
              ),
            )
          ],
        ),
      ),
    );
  }
}
