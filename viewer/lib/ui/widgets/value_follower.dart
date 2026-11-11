import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

/// Settled when within this fraction of the value range of the target: about display
/// precision (under a pixel of a ~120 dp gauge arc or a bar, under the last shown digit
/// of a `%` value), so the ticker stops instead of chasing invisible differences.
const double kFollowerSettleFraction = 0.004;

/// One frame of critically damped exponential smoothing: moves [current] toward
/// [target] by 1 - exp(-dt / tau). Never overshoots; tau <= 0 snaps.
double followStep(double current, double target, Duration dt, Duration tau) {
  if (tau <= Duration.zero) return target;
  if (dt <= Duration.zero) return current;
  final k = 1 - math.exp(-dt.inMicroseconds / tau.inMicroseconds);
  return current + (target - current) * k;
}

/// Follower time constant for a sender interval: 250 ms below 1 s; otherwise
/// 0.6 x interval, capped at 500 ms, so a 1 s sender still glides.
Duration followerTau(Duration interval) {
  if (interval < const Duration(seconds: 1)) {
    return const Duration(milliseconds: 250);
  }
  final t = interval * 0.6;
  const cap = Duration(milliseconds: 500);
  return t > cap ? cap : t;
}

/// Whether [shown] is close enough to [target] to stop animating.
bool followerSettled(double shown, double target, double range) {
  final r = range.isFinite && range > 0 ? range : 1.0;
  return (target - shown).abs() <= r * kFollowerSettleFraction;
}

/// Rebuilds [builder] every frame with a value that glides toward [target]
/// ([followStep]); the ticker stops once settled and restarts on the next target
/// change. Null targets, the first value, and [enabled] == false snap.
class ValueFollower extends StatefulWidget {
  const ValueFollower({
    super.key,
    required this.target,
    required this.tau,
    required this.range,
    required this.builder,
    this.enabled = true,
  });

  final double? target;
  final Duration tau;

  /// Value range (max - min), for the settle threshold.
  final double range;
  final bool enabled;
  final Widget Function(BuildContext context, double? value) builder;

  @override
  State<ValueFollower> createState() => ValueFollowerState();
}

class ValueFollowerState extends State<ValueFollower>
    with SingleTickerProviderStateMixin {
  late final Ticker _ticker = createTicker(_onTick);
  double? _shown;
  Duration _last = Duration.zero;

  /// The value currently displayed.
  double? get shown => _shown;

  /// Whether the follower is animating (for tests).
  bool get isAnimating => _ticker.isActive;

  @override
  void initState() {
    super.initState();
    _shown = widget.target;
  }

  @override
  void didUpdateWidget(ValueFollower oldWidget) {
    super.didUpdateWidget(oldWidget);
    final t = widget.target;
    final s = _shown;
    if (!widget.enabled || t == null || s == null || !t.isFinite) {
      _shown = t;
      if (_ticker.isActive) _ticker.stop();
      return;
    }
    if (followerSettled(s, t, widget.range)) {
      _shown = t;
      if (_ticker.isActive) _ticker.stop();
      return;
    }
    if (!_ticker.isActive) {
      _last = Duration.zero;
      _ticker.start();
    }
  }

  void _onTick(Duration elapsed) {
    final t = widget.target;
    final s = _shown;
    if (t == null || s == null) {
      _ticker.stop();
      return;
    }
    final dt = elapsed - _last;
    _last = elapsed;
    var next = followStep(s, t, dt, widget.tau);
    if (followerSettled(next, t, widget.range)) {
      next = t;
      _ticker.stop();
    }
    setState(() => _shown = next);
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, _shown);
}

/// A 0..1 "scroll phase" for line graphs: time since the last sample, as a fraction of
/// the sender interval. One driver serves every graph of a card; painters take it as
/// their `repaint` listenable, so a tick repaints the plots without rebuilding widgets.
/// Ticks only until the phase reaches 1 (a late packet).
class ScrollPhaseDriver extends ChangeNotifier
    implements ValueListenable<double> {
  ScrollPhaseDriver(TickerProvider vsync) {
    _ticker = vsync.createTicker(_onTick);
  }

  late final Ticker _ticker;
  Duration _interval = const Duration(seconds: 1);
  double _value = 0;

  @override
  double get value => _value;

  /// Whether the driver is ticking (for tests).
  bool get isActive => _ticker.isActive;

  /// A new sample arrived: restart the phase at 0 and tick toward 1 over [interval].
  void restart(Duration interval) {
    _interval = interval;
    if (_ticker.isActive) _ticker.stop();
    _set(0);
    _ticker.start();
  }

  /// Stop and hold the phase at 0 (smooth animation off).
  void reset() {
    if (_ticker.isActive) _ticker.stop();
    _set(0);
  }

  void _onTick(Duration elapsed) {
    final us = _interval.inMicroseconds;
    var p = us <= 0 ? 1.0 : elapsed.inMicroseconds / us;
    if (p >= 1) {
      p = 1;
      _ticker.stop();
    }
    _set(p);
  }

  void _set(double v) {
    if (v == _value) return;
    _value = v;
    notifyListeners();
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }
}

/// Owns a [ScrollPhaseDriver] restarted whenever [sampleKey] changes (or on re-enable).
/// [builder] runs on widget rebuilds only, not per tick: pass the phase on to painters.
class ScrollPhase extends StatefulWidget {
  const ScrollPhase({
    super.key,
    required this.sampleKey,
    required this.interval,
    required this.builder,
    this.enabled = true,
  });

  final Object? sampleKey;
  final Duration interval;
  final bool enabled;
  final Widget Function(BuildContext context, ValueListenable<double> phase)
  builder;

  @override
  State<ScrollPhase> createState() => _ScrollPhaseState();
}

class _ScrollPhaseState extends State<ScrollPhase>
    with SingleTickerProviderStateMixin {
  late final ScrollPhaseDriver _driver = ScrollPhaseDriver(this);

  @override
  void initState() {
    super.initState();
    if (widget.enabled) _driver.restart(widget.interval);
  }

  @override
  void didUpdateWidget(ScrollPhase oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!widget.enabled) {
      _driver.reset();
      return;
    }
    // Restart only on a new sample (or re-enable); other rebuilds must not reset it.
    if (widget.sampleKey != oldWidget.sampleKey || !oldWidget.enabled) {
      _driver.restart(widget.interval);
    }
  }

  @override
  void dispose() {
    _driver.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, _driver);
}
