import 'package:flutter/material.dart';

/// A two-widget tab view used to display the call controls and chat widget in a single column.
class HomeTabView extends StatefulWidget {
  final Widget widgetOne;
  final Widget widgetTwo;
  final Color colorOne;
  final Color colorTwo;
  final Icon iconOne;
  final Icon iconTwo;

  const HomeTabView(
      {super.key,
      required this.widgetOne,
      required this.widgetTwo,
      required this.colorOne,
      required this.colorTwo,
      required this.iconOne,
      required this.iconTwo});

  @override
  State<HomeTabView> createState() => HomeTabViewState();
}

class HomeTabViewState extends State<HomeTabView>
    with SingleTickerProviderStateMixin {
  late TabController _tabController;
  late Color _backgroundColor = widget.colorOne;

  @override
  void initState() {
    super.initState();
    _tabController = TabController(length: 2, vsync: this);
    _tabController.addListener(() {
      setState(() {
        _backgroundColor =
            _tabController.index == 0 ? widget.colorOne : widget.colorTwo;
      });
    });
  }

  @override
  void dispose() {
    _tabController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return DefaultTabController(
      length: 2,
      child: Flexible(
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
                    widget.iconOne,
                    widget.iconTwo,
                  ],
                ),
              ),
              Flexible(
                child: TabBarView(
                  controller: _tabController,
                  children: [
                    widget.widgetOne,
                    widget.widgetTwo,
                  ],
                ),
              )
            ],
          ),
        ),
      ),
    );
  }
}
