//! Stroke-to-fill conversion algorithm.
//!
//! [`Stroker`] converts a stroked path into a fillable outline using the same
//! [`StrokeOptions`] as the stroke tessellator. See [`StrokeOptions`].
//!
//! The output has self-overlapping contours and must be filled
//! with the non-zero fill rule.
//!
//! # Examples
//!
//! ```
//! use lyon_algorithms::path::Path;
//! use lyon_algorithms::path::math::point;
//! use lyon_algorithms::stroke::{LineCap, LineJoin, StrokeOptions, Stroker};
//!
//! # fn main() {
//! let mut builder = Path::builder();
//! builder.begin(point(0.0, 0.0));
//! builder.line_to(point(100.0, 0.0));
//! builder.line_to(point(100.0, 100.0));
//! builder.end(false);
//! let source = builder.build();
//!
//! let options = StrokeOptions::new(10.0)
//!     .with_line_join(LineJoin::Round)
//!     .with_line_cap(LineCap::Butt);
//!
//! // The output is a single contour going along one side of the path, around
//! // the end cap, back along the other side and around the start cap.
//! let mut builder = Path::builder();
//! let mut stroker = Stroker::new();
//! stroker.stroke_to_fill(source.iter(), &options, &mut builder);
//! let outline = builder.build();
//! # }
//! ```

use alloc::vec::Vec;

// In std builds, `tan` comes from the inherent f32 method. It is only provided
// by the trait in no_std builds.
#[cfg(not(feature = "std"))]
use num_traits::Float;
use path::{builder::PathBuilder, Attributes, EndpointId, PathEvent, NO_ATTRIBUTES};
use path::math::{Point, Vector};

pub use crate::offset::{LineCap, LineJoin};
use crate::offset::Offsetter;

/// The parameters of a stroke, shared by the triangle mesh and the fillable
/// contour.
///
/// The stroke tessellator (`lyon_tessellation`) and the stroke-to-fill
/// conversion ([`Stroker`]) both take their line width, miter limit, join,
/// end caps and tolerance from this single structure. Keeping one copy means
/// the mesh and the hit-testing contour always describe the same stroke.
///
/// The stroke-to-fill output has self-overlapping contours and must be filled
/// with the non-zero fill rule.
///
/// In addition to a symmetric line width, asymmetric strokes can be specified
/// with [`StrokeOptions::with_offsets`]. Per-endpoint varying widths are
/// driven by a custom path attribute, selected with
/// [`StrokeOptions::with_variable_line_width`]; the tessellator interpolates
/// the width along the path while the stroke-to-fill conversion ignores it
/// (it has no attributes on its input builder).
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialization", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct StrokeOptions {
    /// What cap to use at the start of each sub-path.
    ///
    /// Default value: `LineCap::Butt`.
    pub start_cap: LineCap,

    /// What cap to use at the end of each sub-path.
    ///
    /// Default value: `LineCap::Butt`.
    pub end_cap: LineCap,

    /// How segments of the stroke are joined together.
    ///
    /// Default value: `LineJoin::Miter`.
    pub line_join: LineJoin,

    /// Line width.
    ///
    /// Default value: `StrokeOptions::DEFAULT_LINE_WIDTH`.
    pub line_width: f32,

    /// Index of a custom attribute defining a per-endpoint factor that
    /// modulates the line width. The actual width at an endpoint is
    /// `line_width` multiplied by this factor.
    ///
    /// Only the stroke tessellator consumes custom attributes; the
    /// stroke-to-fill conversion does not vary the width.
    ///
    /// Default value: `None`.
    pub variable_line_width: Option<path::AttributeIndex>,

    /// See the SVG specification.
    ///
    /// Must be greater than or equal to 1.0.
    /// Default value: `StrokeOptions::DEFAULT_MITER_LIMIT`.
    pub miter_limit: f32,

    /// Maximum allowed distance to the path when approximating curves.
    ///
    /// Default value: `StrokeOptions::DEFAULT_TOLERANCE`.
    pub tolerance: f32,

    /// Distances from the source path to each side of the stroke, when they
    /// differ. When `None`, both sides sit at `line_width / 2`.
    pub(crate) offsets: Option<(f32, f32)>,

    /// Whether zero-length sub-paths draw their caps around their point.
    pub(crate) add_empty_caps: bool,
}

impl StrokeOptions {
    /// Minimum miter limit as defined by the SVG specification.
    ///
    /// See [StrokeMiterLimitProperty](https://svgwg.org/specs/strokes/#StrokeMiterlimitProperty)
    pub const MINIMUM_MITER_LIMIT: f32 = 1.0;
    /// Default miter limit as defined by the SVG specification.
    ///
    /// See [StrokeMiterLimitProperty](https://svgwg.org/specs/strokes/#StrokeMiterlimitProperty)
    pub const DEFAULT_MITER_LIMIT: f32 = 4.0;
    pub const DEFAULT_LINE_CAP: LineCap = LineCap::Butt;
    pub const DEFAULT_LINE_JOIN: LineJoin = LineJoin::Miter;
    pub const DEFAULT_LINE_WIDTH: f32 = 1.0;
    pub const DEFAULT_TOLERANCE: f32 = 0.1;

    pub const DEFAULT: Self = StrokeOptions {
        start_cap: Self::DEFAULT_LINE_CAP,
        end_cap: Self::DEFAULT_LINE_CAP,
        line_join: Self::DEFAULT_LINE_JOIN,
        line_width: Self::DEFAULT_LINE_WIDTH,
        variable_line_width: None,
        miter_limit: Self::DEFAULT_MITER_LIMIT,
        tolerance: Self::DEFAULT_TOLERANCE,
        offsets: None,
        add_empty_caps: true,
    };

    #[inline]
    pub fn new(width: f32) -> Self {
        Self::DEFAULT.with_line_width(width)
    }

    #[inline]
    pub fn tolerance(tolerance: f32) -> Self {
        Self::DEFAULT.with_tolerance(tolerance)
    }

    #[inline]
    pub fn with_tolerance(mut self, tolerance: f32) -> Self {
        self.tolerance = tolerance;
        self
    }

    #[inline]
    pub fn with_line_cap(mut self, cap: LineCap) -> Self {
        self.start_cap = cap;
        self.end_cap = cap;
        self
    }

    #[inline]
    pub fn with_start_cap(mut self, cap: LineCap) -> Self {
        self.start_cap = cap;
        self
    }

    #[inline]
    pub fn with_end_cap(mut self, cap: LineCap) -> Self {
        self.end_cap = cap;
        self
    }

    #[inline]
    pub fn with_line_join(mut self, join: LineJoin) -> Self {
        self.line_join = join;
        self
    }

    #[inline]
    pub fn with_line_width(mut self, width: f32) -> Self {
        self.line_width = width;
        self.offsets = None;
        self
    }

    /// Instead of using a symmetric line width along the path, specify
    /// two offsets, one for each side.
    ///
    /// `StrokeOptions::with_line_width(w)` is equivalent to
    /// `StrokeOptions::with_offsets(-w * 0.5, w * 0.5)`.
    #[inline]
    pub fn with_offsets(mut self, first: f32, second: f32) -> Self {
        self.offsets = Some((first, second));
        self.line_width = (second - first).abs();
        self
    }

    /// Sets the miter limit, clamped to at least
    /// [`StrokeOptions::MINIMUM_MITER_LIMIT`].
    #[inline]
    pub fn with_miter_limit(mut self, limit: f32) -> Self {
        self.miter_limit = limit.max(Self::MINIMUM_MITER_LIMIT);
        self
    }

    /// Selects a custom attribute whose value modulates the line width at
    /// each endpoint. The tessellator interpolates the factor between
    /// endpoints; the stroke-to-fill conversion ignores it.
    #[inline]
    pub fn with_variable_line_width(mut self, attribute_index: path::AttributeIndex) -> Self {
        self.variable_line_width = Some(attribute_index);
        self
    }

    /// Whether zero-length sub-paths draw their caps around their point.
    ///
    /// Default value: `true`.
    #[inline]
    pub fn with_empty_caps(mut self, add_empty_caps: bool) -> Self {
        self.add_empty_caps = add_empty_caps;
        self
    }

    /// The offset to each side of the source path.
    #[inline]
    pub(crate) fn side_offsets(&self) -> (f32, f32) {
        self.offsets
            .unwrap_or_else(|| (-self.line_width * 0.5, self.line_width * 0.5))
    }
}

impl Default for StrokeOptions {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Deprecated name for [`StrokeOptions`].
///
/// The stroke tessellator and the stroke-to-fill conversion now share a single
/// set of options, so `StrokeToFillOptions` is kept only as an alias.
pub type StrokeToFillOptions = StrokeOptions;

/// A context object that can build the contour of a stroked path.
///
/// Reusing the same `Stroker` can avoid memory allocations.
#[derive(Default)]
pub struct Stroker {
    /// Storage for StrokeToFillBuilder::opposite, kept between calls to avoid
    /// reallocating it.
    opposite: SegmentBuffer,
}

impl Stroker {
    pub fn new() -> Stroker {
        Stroker {
            opposite: SegmentBuffer::default(),
        }
    }

    pub fn with_capacity(cap: usize) -> Stroker {
        Stroker {
            opposite: SegmentBuffer {
                start: Default::default(),
                segments: Vec::with_capacity(cap),
            }
        }
    }

    pub fn capacity(&self) -> usize {
        self.opposite.segments.len()
    }

    /// Return a `PathBuilder` that computes the stroke's contour.
    pub fn builder<'l>(
        &'l mut self,
        output: &'l mut dyn PathBuilder,
        options: &StrokeOptions
    ) -> StrokeToFillBuilder {
        StrokeToFillBuilder::new(self, output, options)
    }

    /// Compute the contour of a stroked path.
    ///
    /// The produced paths self-overlap and must be filled with the non-zero
    /// fill rule to properly represent the stroke.
    pub fn stroke_to_fill(
        &mut self,
        input: impl Iterator<Item = PathEvent>,
        options: &StrokeOptions,
        output: &mut dyn PathBuilder,
    ) {
        let mut builder = self.builder(output, options);

        for event in input {
            match event {
                PathEvent::Begin { at } => {
                    builder.begin(at);
                }
                PathEvent::End { close, .. } => {
                    builder.end(close);
                }
                PathEvent::Line { to, .. } => {
                    builder.line_to(to);
                }
                PathEvent::Quadratic { ctrl, to, .. } => {
                    builder.quadratic_bezier_to(ctrl, to);
                }
                PathEvent::Cubic {
                    ctrl1, ctrl2, to, ..
                } => {
                    builder.cubic_bezier_to(ctrl1, ctrl2, to);
                }
            }
        }
    }
}

/// Computes the contour of a stroked path using the `PathBuilder` API.
///
/// See [`Stroker::builder`].
///
/// This builder does not support custom attributes.
pub struct StrokeToFillBuilder<'l> {
    stroker: &'l mut Stroker,
    output: &'l mut dyn PathBuilder,
    /// The second side of the stroke, in the direction of the source path. It
    /// is added to the output in reverse order at the end of each sub-path.
    opposite: SegmentBuffer,
    offsetter: Offsetter<2>,
    offsets: (f32, f32),
    start_cap: LineCap,
    end_cap: LineCap,
    add_empty_caps: bool,
}

impl<'l> StrokeToFillBuilder<'l> {
    fn new(
        stroker: &'l mut Stroker,
        output: &'l mut dyn PathBuilder,
        options: &StrokeOptions
    ) -> Self {
        let offsets = options.side_offsets();
        let mut offsetter = Offsetter::new(
            [offsets.0, offsets.1],
            options.line_join,
            options.miter_limit,
            options.tolerance,
            true,
        );
        // Open sub-paths get caps even if they end at their start point.
        offsetter.close_if_end_at_start = false;

        let opposite = core::mem::take(&mut stroker.opposite);

        StrokeToFillBuilder {
            stroker,
            output,
            opposite,
            offsetter,
            offsets,
            start_cap: options.start_cap,
            end_cap: options.end_cap,
            add_empty_caps: options.add_empty_caps,
        }
    }

    #[inline]
    pub fn begin(&mut self, at: Point) {
        self.offsetter.begin(at);
    }

    #[inline]
    pub fn line_to(&mut self, to: Point) {
        let mut outputs: [&mut dyn PathBuilder; 2] = [&mut *self.output, &mut self.opposite];
        self.offsetter.line_to(to, &mut outputs);
    }

    #[inline]
    pub fn quadratic_bezier_to(&mut self, ctrl: Point, to: Point) {
        let mut outputs: [&mut dyn PathBuilder; 2] = [&mut *self.output, &mut self.opposite];
        self.offsetter.quadratic_bezier_to(ctrl, to, &mut outputs);
    }

    #[inline]
    pub fn cubic_bezier_to(&mut self, ctrl1: Point, ctrl2: Point, to: Point) {
        let mut outputs: [&mut dyn PathBuilder; 2] = [&mut *self.output, &mut self.opposite];
        self.offsetter.cubic_bezier_to(ctrl1, ctrl2, to, &mut outputs);
    }

    pub fn end(&mut self, close: bool) {
        let mut outputs: [&mut dyn PathBuilder; 2] = [&mut *self.output, &mut self.opposite];
        let Some(closed) = self.offsetter.end(close, &mut outputs) else {
            if self.add_empty_caps {
                self.add_empty_cap();
            }
            return;
        };

        if closed {
            // Each side is a separate contour.
            self.output.end(true);
            self.output.begin(self.opposite.last_point(), NO_ATTRIBUTES);
            self.opposite.add_reversed_to(&mut *self.output);
            self.output.end(true);
        } else {
            // A single contour that goes along one side, around the end cap, back
            // along the other side and around the start cap.
            self.add_end_cap();
            self.opposite.add_reversed_to(&mut *self.output);
            self.add_start_cap();
            self.output.end(true);
        }
    }

    fn add_start_cap(&mut self) {
        let endpoint = self.offsetter.first_endpoint;
        let tangent = -self.offsetter.first_tangent;
        let n = normal(tangent);
        let width = (self.offsets.0 - self.offsets.1).abs() * 0.5;
        let pivot_offset = (self.offsets.0 + self.offsets.1) * 0.5;
        let pivot = endpoint - n * pivot_offset;
        let end = endpoint - n * self.offsets.0;
        let sign = (self.offsets.0 - self.offsets.1).signum();
        get_cap_fn(self.start_cap)(
            &mut *self.output,
            pivot,
            tangent,
            end,
            width,
            sign,
            self.offsetter.tolerance,
        );
    }

    fn add_end_cap(&mut self) {
        let tangent = self.offsetter.prev_tangent;
        let n = normal(tangent);
        let width = (self.offsets.0 - self.offsets.1).abs() * 0.5;
        let pivot_offset = (self.offsets.0 + self.offsets.1) * 0.5;
        let pivot = self.offsetter.prev_endpoint + n * pivot_offset;
        let end = self.offsetter.prev_endpoint + n * self.offsets.1;
        let sign = (self.offsets.0 - self.offsets.1).signum();
        get_cap_fn(self.end_cap)(
            &mut *self.output,
            pivot,
            tangent,
            end,
            width,
            sign,
            self.offsetter.tolerance,
        );
    }

    fn add_empty_cap(&mut self) {
        // Inverted caps carve into the stroke, and a zero-length sub-path has
        // no stroke to carve into, so they act like butt caps here.
        let empty_cap = |cap| match cap {
            LineCap::RoundInverted | LineCap::TriangleInverted => LineCap::Butt,
            cap => cap,
        };
        let start_cap = empty_cap(self.start_cap);
        let end_cap = empty_cap(self.end_cap);

        // Butt caps have no area, so a zero-length sub-path with them draws
        // nothing.
        if start_cap == LineCap::Butt && end_cap == LineCap::Butt {
            return;
        }

        let tangent = Vector::new(1.0, 0.0);
        let normal = normal(tangent);

        // We don't have a real tangent so we don't offset the pivot.
        let pivot = self.offsetter.first_endpoint;

        let width = (self.offsets.0 - self.offsets.1).abs() * 0.5;
        let p0 = pivot + normal * width;
        let p1 = pivot - normal * width;

        let tolerance = self.offsetter.tolerance;
        let output = &mut *self.output;
        output.begin(p0, NO_ATTRIBUTES);
        get_cap_fn(end_cap)(output, pivot, tangent, p1, width, 1.0, tolerance);
        get_cap_fn(start_cap)(output, pivot, -tangent, p0, width, 1.0, tolerance);
        output.end(true);
    }
}

impl <'l> Drop for StrokeToFillBuilder<'l> {
    fn drop(&mut self) {
        core::mem::swap(&mut self.opposite, &mut self.stroker.opposite);
    }
}

impl<'a> PathBuilder for StrokeToFillBuilder<'a> {
    fn num_attributes(&self) -> usize {
        0
    }

    fn begin(&mut self, at: Point, _: Attributes) -> EndpointId {
        self.begin(at);
        EndpointId::INVALID
    }

    fn end(&mut self, close: bool) {
        self.end(close);
    }

    fn line_to(&mut self, to: Point, _: Attributes) -> EndpointId {
        self.line_to(to);
        EndpointId::INVALID
    }

    fn quadratic_bezier_to(&mut self, ctrl: Point, to: Point, _: Attributes) -> EndpointId {
        self.quadratic_bezier_to(ctrl, to);
        EndpointId::INVALID
    }

    fn cubic_bezier_to(
        &mut self,
        ctrl1: Point,
        ctrl2: Point,
        to: Point,
        _: Attributes,
    ) -> EndpointId {
        self.cubic_bezier_to(ctrl1, ctrl2, to);
        EndpointId::INVALID
    }
}

fn get_cap_fn(cap: LineCap) -> CapBuilder {
    match cap {
        LineCap::Butt => butt_cap,
        LineCap::Square => square_cap,
        LineCap::Round => round_cap_cubic,
        LineCap::RoundInverted => round_inverted_cap,
        LineCap::Triangle => triangle_cap,
        LineCap::TriangleInverted => triangle_inverted_cap,
    }
}

#[derive(Copy, Clone)]
enum RecordedSegment {
    Line { to: Point },
    Quadratic { ctrl: Point, to: Point },
    Cubic { ctrl1: Point, ctrl2: Point, to: Point },
}

impl RecordedSegment {
    fn to(&self) -> Point {
        match *self {
            RecordedSegment::Line { to }
            | RecordedSegment::Quadratic { to, .. }
            | RecordedSegment::Cubic { to, .. } => to,
        }
    }
}

/// Records a sub-path, to add it to another path in reverse order.
#[derive(Default)]
struct SegmentBuffer {
    start: Point,
    segments: Vec<RecordedSegment>,
}

impl SegmentBuffer {
    fn last_point(&self) -> Point {
        self.segments.last().map_or(self.start, RecordedSegment::to)
    }

    /// Adds the segments to `output` in reverse order, starting from the
    /// current position of `output`.
    fn add_reversed_to(&self, output: &mut dyn PathBuilder) {
        // Each segment, reversed, ends where the previous one starts.
        for (i, segment) in self.segments.iter().enumerate().rev() {
            let to = match i {
                0 => self.start,
                _ => self.segments[i - 1].to(),
            };
            match *segment {
                RecordedSegment::Line { .. } => {
                    output.line_to(to, NO_ATTRIBUTES);
                }
                RecordedSegment::Quadratic { ctrl, .. } => {
                    output.quadratic_bezier_to(ctrl, to, NO_ATTRIBUTES);
                }
                RecordedSegment::Cubic { ctrl1, ctrl2, .. } => {
                    output.cubic_bezier_to(ctrl2, ctrl1, to, NO_ATTRIBUTES);
                }
            }
        }
    }
}

impl PathBuilder for SegmentBuffer {
    fn num_attributes(&self) -> usize {
        0
    }

    fn begin(&mut self, at: Point, _: Attributes) -> EndpointId {
        self.start = at;
        self.segments.clear();
        EndpointId::INVALID
    }

    fn end(&mut self, _close: bool) {}

    fn line_to(&mut self, to: Point, _: Attributes) -> EndpointId {
        self.segments.push(RecordedSegment::Line { to });
        EndpointId::INVALID
    }

    fn quadratic_bezier_to(&mut self, ctrl: Point, to: Point, _: Attributes) -> EndpointId {
        self.segments.push(RecordedSegment::Quadratic { ctrl, to });
        EndpointId::INVALID
    }

    fn cubic_bezier_to(&mut self, ctrl1: Point, ctrl2: Point, to: Point, _: Attributes) -> EndpointId {
        self.segments.push(RecordedSegment::Cubic { ctrl1, ctrl2, to });
        EndpointId::INVALID
    }
}

pub(crate) type CapBuilder = fn(
    output: &mut dyn PathBuilder,
    position: Point,
    tangent: Vector,
    end: Point,
    width: f32,
    sign: f32,
    tolerance: f32,
);

fn butt_cap(
    output: &mut dyn PathBuilder,
    _position: Point,
    _tangent: Vector,
    end: Point,
    _width: f32,
    _sign: f32,
    _tolerance: f32,
) {
    output.line_to(end, NO_ATTRIBUTES);
}

fn square_cap(
    output: &mut dyn PathBuilder,
    pivot: Point,
    tangent: Vector,
    end: Point,
    width: f32,
    sign: f32,
    _tolerance: f32,
) {
    let tw = tangent * width;
    let n = normal(tw) * sign;
    let mid = pivot + tw;
    let a = mid + n;
    let b = mid - n;

    output.line_to(a, NO_ATTRIBUTES);
    output.line_to(b, NO_ATTRIBUTES);
    output.line_to(end, NO_ATTRIBUTES);
}

fn round_cap_cubic(
    output: &mut dyn PathBuilder,
    pivot: Point,
    tangent: Vector,
    end: Point,
    width: f32,
    sign: f32,
    _tolerance: f32,
) {
    let normal = normal(tangent) * sign;
    let start = pivot + normal * width;

    // From https://pomax.github.io/bezierinfo/#circles
    let angle = core::f32::consts::PI * 0.5;
    let k = width.abs() * 4.0 / 3.0 * (angle * 0.25).tan();

    let mid = pivot + tangent * width;
    let ctrl1 = start + tangent * k;
    let ctrl2 = mid + normal * k;
    output.cubic_bezier_to(ctrl1, ctrl2, mid, NO_ATTRIBUTES);
    let ctrl1 = mid - normal * k;
    let ctrl2 = end + tangent * k;
    output.cubic_bezier_to(ctrl1, ctrl2, end, NO_ATTRIBUTES);
}

fn round_inverted_cap(
    output: &mut dyn PathBuilder,
    pivot: Point,
    tangent: Vector,
    end: Point,
    width: f32,
    sign: f32,
    _tolerance: f32,
) {
    round_cap_cubic(output, pivot, -tangent, end, width, -sign, _tolerance);
}

fn triangle_cap(
    output: &mut dyn PathBuilder,
    pivot: Point,
    tangent: Vector,
    end: Point,
    width: f32,
    _sign: f32,
    _tolerance: f32,
) {
    let tw = tangent * width;
    let mid = pivot + tw;

    output.line_to(mid, NO_ATTRIBUTES);
    output.line_to(end, NO_ATTRIBUTES);
}

fn triangle_inverted_cap(
    output: &mut dyn PathBuilder,
    pivot: Point,
    tangent: Vector,
    end: Point,
    width: f32,
    sign: f32,
    tolerance: f32,
) {
    triangle_cap(output, pivot, -tangent, end, width, -sign, tolerance)
}

fn normal(v: Vector) -> Vector {
    Vector::new(-v.y, v.x)
}
