import 'dart:math' as math;

import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

/// Settled when within this fraction of the value range of the target.
const double kFollowerSettleFraction = 0.0005;

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

/// Drives a 0..1 "scroll phase" for line graphs: time since [sampleKey] last changed,
/// as a fraction of [interval]. Ticks only until the phase reaches 1 (a late packet),
/// and not at all when [enabled] is false (phase stays 0).
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
  final Widget Function(BuildContext context, double phase) builder;

  @override
  State<ScrollPhase> createState() => _ScrollPhaseState();
}

class _ScrollPhaseState extends State<ScrollPhase>
    with SingleTickerProviderStateMixin {
  late final Ticker _ticker = createTicker(_onTick);
  double _phase = 0;

  @override
  void initState() {
    super.initState();
    if (widget.enabled) _ticker.start();
  }

  @override
  void didUpdateWidget(ScrollPhase oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!widget.enabled) {
      _phase = 0;
      if (_ticker.isActive) _ticker.stop();
      return;
    }
    // Restart only on a new sample (or re-enable); other rebuilds must not reset it.
    if (widget.sampleKey != oldWidget.sampleKey || !oldWidget.enabled) {
      _phase = 0;
      if (_ticker.isActive) _ticker.stop();
      _ticker.start(); // elapsed restarts at 0 = this sample's arrival
    }
  }

  void _onTick(Duration elapsed) {
    final ms = widget.interval.inMicroseconds;
    var p = ms <= 0 ? 1.0 : elapsed.inMicroseconds / ms;
    if (p >= 1) {
      p = 1;
      _ticker.stop();
    }
    setState(() => _phase = p);
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, _phase);
}
