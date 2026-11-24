//! The stroke tessellator (triangle mesh) and the stroke-to-fill conversion
//! (fillable contour used for hit-testing) consume the same
//! [`lyon_algorithms::stroke::StrokeOptions`]. These tests pin down, for a
//! collection of representative paths, that the mesh and the contour both
//! come out of that single set of options and keep their own data (the
//! mesh's advancement, normals and varying width, and the contour's caps).

// The assertions pin down concrete f32 values produced by the algorithms.
#![allow(clippy::excessive_precision, clippy::approx_constant)]

use lyon_algorithms::hit_test::hit_test_path;
use lyon_algorithms::stroke::{LineCap, LineJoin, StrokeOptions, Stroker};
use lyon_path::math::{Point, Vector, point};
use lyon_path::{FillRule, Path};
use lyon_tessellation::geometry_builder::{BuffersBuilder, VertexBuffers, simple_builder};
use lyon_tessellation::{
    FillOptions, FillTessellator, StrokeTessellator, StrokeVertex, StrokeVertexConstructor,
};

const EPS: f32 = 1e-4;

#[derive(Copy, Clone, Debug)]
struct Vtx {
    position: Point,
    normal: Vector,
    advancement: f32,
    line_width: f32,
}

struct Ctor;
impl StrokeVertexConstructor<Vtx> for Ctor {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> Vtx {
        Vtx {
            position: vertex.position(),
            normal: vertex.normal(),
            advancement: vertex.advancement(),
            line_width: vertex.line_width(),
        }
    }
}

fn mesh(path: &Path, options: &StrokeOptions) -> VertexBuffers<Vtx, u16> {
    let mut buffers: VertexBuffers<Vtx, u16> = VertexBuffers::new();
    {
        let mut builder = BuffersBuilder::new(&mut buffers, Ctor);
        let mut tess = StrokeTessellator::new();
        tess.tessellate_path(path, options, &mut builder).unwrap();
    }
    buffers
}

fn outline(path: &Path, options: &StrokeOptions) -> Path {
    let mut builder = Path::builder();
    Stroker::new().stroke_to_fill(path.iter(), options, &mut builder);
    builder.build()
}

fn l_path() -> Path {
    let mut b = Path::builder();
    b.begin(point(0.0, 0.0));
    b.line_to(point(100.0, 0.0));
    b.line_to(point(100.0, 100.0));
    b.end(false);
    b.build()
}

fn assert_near(actual: Point, expected: Point) {
    assert!(
        (actual.x - expected.x).abs() < EPS && (actual.y - expected.y).abs() < EPS,
        "expected {expected:?}, got {actual:?}"
    );
}

fn assert_contains_point(path: &Path, expected: Point) {
    let mut points = path.iter().flat_map(|event| match event {
        lyon_path::PathEvent::Line { to, .. } => vec![to],
        lyon_path::PathEvent::Quadratic { ctrl, to, .. } => vec![ctrl, to],
        lyon_path::PathEvent::Cubic {
            ctrl1, ctrl2, to, ..
        } => vec![ctrl1, ctrl2, to],
        lyon_path::PathEvent::Begin { at } => vec![at],
        lyon_path::PathEvent::End { .. } => vec![],
    });
    let found = points.any(|p| (p.x - expected.x).abs() < EPS && (p.y - expected.y).abs() < EPS);
    assert!(found, "outline does not contain {expected:?}");
}

#[test]
fn right_angle_miter_clip_limit_one() {
    let path = l_path();
    let options = StrokeOptions::new(10.0)
        .with_line_join(LineJoin::MiterClip)
        .with_miter_limit(1.0)
        .with_line_cap(LineCap::Butt)
        .with_tolerance(0.1);

    let m = mesh(&path, &options);
    assert_eq!((m.vertices.len(), m.indices.len()), (6, 12));
    assert_eq!(&m.indices[..], &[0, 1, 2, 0, 2, 3, 5, 4, 1, 5, 1, 0]);

    let outer = m.vertices[0];
    assert_near(outer.position, point(105.0, -5.0));
    assert!((outer.normal.x - 1.0).abs() < EPS && (outer.normal.y + 1.0).abs() < EPS);
    assert_eq!(outer.advancement, 100.0);
    assert_eq!(m.vertices[2].advancement, 200.0);

    // The mesh keeps its unclipped miter while the contour clips the outer
    // corner, both driven by the same options.
    let o = outline(&path, &options);
    assert_contains_point(&o, point(102.071068, -5.0));
    assert_contains_point(&o, point(105.0, -2.071068));

    // The contour is the hit-testing shape, filled with the non-zero rule.
    let hit = |x: f32, y: f32| {
        hit_test_path(&point(x, y), o.iter(), FillRule::NonZero, options.tolerance)
    };
    assert!(hit(50.0, 0.0));
    assert!(hit(100.0, 50.0));
    assert!(!hit(50.0, 50.0));
    assert!(!hit(105.0, -5.0));
}

#[test]
fn right_angle_miter_limit_four() {
    let path = l_path();
    let options = StrokeOptions::new(10.0)
        .with_line_join(LineJoin::Miter)
        .with_miter_limit(4.0)
        .with_line_cap(LineCap::Butt)
        .with_tolerance(0.1);

    let m = mesh(&path, &options);
    // With the miter limit at four, the contour's outer vertex coincides with
    // the mesh's.
    assert_near(m.vertices[0].position, point(105.0, -5.0));
    assert_near(m.vertices[1].position, point(95.0, 5.0));
    let o = outline(&path, &options);
    assert_contains_point(&o, point(105.0, -5.0));
    assert_contains_point(&o, point(95.0, 5.0));
}

#[test]
fn variable_line_width() {
    let mut b = Path::builder_with_attributes(1);
    b.begin(point(0.0, 0.0), &[0.3]);
    b.line_to(point(80.0, 0.0), &[1.0]);
    b.end(false);
    let path = b.build();

    let options = StrokeOptions::new(10.0)
        .with_line_cap(LineCap::Butt)
        .with_line_join(LineJoin::Miter)
        .with_variable_line_width(0)
        .with_tolerance(0.1);

    let m = mesh(&path, &options);
    assert_eq!((m.vertices.len(), m.indices.len()), (4, 6));
    assert_eq!(&m.indices[..], &[3, 2, 0, 3, 0, 1]);

    // The varying width lives in the mesh: width readings, normals and
    // advancement. The edge lines are not perpendicular to the path but the
    // cap still lands on the end line.
    let (start_a, start_b) = (m.vertices[2], m.vertices[3]);
    assert_near(start_a.position, point(0.0, 1.501438));
    assert_near(start_b.position, point(0.0, -1.501438));
    assert_eq!(start_a.line_width, 3.0);
    assert!((start_a.normal.y - 1.000958).abs() < EPS);
    assert!((start_b.normal.y + 1.000958).abs() < EPS);
    assert_eq!(start_a.advancement, 0.0);

    let (end_a, end_b) = (m.vertices[0], m.vertices[1]);
    assert_near(end_a.position, point(80.0, 5.004792));
    assert_near(end_b.position, point(80.0, -5.004792));
    assert_eq!(end_a.line_width, 10.0);
    assert!((end_a.normal.y - 1.000958).abs() < EPS);
    assert_eq!(end_a.advancement, 80.0);
}

#[test]
fn closed_square() {
    let mut b = Path::builder();
    b.begin(point(0.0, 0.0));
    b.line_to(point(100.0, 0.0));
    b.line_to(point(100.0, 100.0));
    b.line_to(point(0.0, 100.0));
    b.end(true);
    let path = b.build();

    let options = StrokeOptions::new(10.0)
        .with_line_join(LineJoin::Miter)
        .with_miter_limit(4.0)
        .with_line_cap(LineCap::Butt)
        .with_tolerance(0.1);

    let m = mesh(&path, &options);
    assert_eq!((m.vertices.len(), m.indices.len()), (10, 24));
    assert_near(m.vertices[2].position, point(105.0, 105.0));
    assert_eq!(m.vertices[2].advancement, 200.0);
    assert_near(m.vertices[6].position, point(-5.0, -5.0));
    assert_eq!(m.vertices[6].advancement, 400.0);
    // The duplicated pair at advancement zero shares the closing corner.
    assert_near(m.vertices[8].position, point(5.0, 5.0));
    assert_near(m.vertices[9].position, point(-5.0, -5.0));
    assert_eq!(m.vertices[8].advancement, 0.0);
    assert_eq!(m.vertices[9].advancement, 0.0);

    let o = outline(&path, &options);
    for corner in [
        point(105.0, -5.0),
        point(105.0, 105.0),
        point(-5.0, 105.0),
        point(-5.0, -5.0),
    ] {
        assert_contains_point(&o, corner);
    }
    assert_contains_point(&o, point(4.999998, 95.0));
    assert_contains_point(&o, point(95.0, 95.000008));
}

#[test]
fn round_caps_and_joins() {
    let path = l_path();
    let options = StrokeOptions::new(10.0)
        .with_line_join(LineJoin::Round)
        .with_line_cap(LineCap::Round)
        .with_tolerance(0.1);

    let m = mesh(&path, &options);
    assert_eq!((m.vertices.len(), m.indices.len()), (24, 66));
    let join_vertex = m
        .vertices
        .iter()
        .find(|v| (v.position.x - 103.535538).abs() < EPS && (v.position.y + 3.535534).abs() < EPS)
        .expect("round join vertex");
    assert!((join_vertex.normal.x - 0.707107).abs() < EPS);
    assert!((join_vertex.normal.y + 0.707107).abs() < EPS);
    assert_eq!(join_vertex.advancement, 100.0);
    // The start round cap extends past the path.
    assert!(
        m.vertices
            .iter()
            .any(|v| (v.position - point(-5.0, 0.0)).length() < EPS)
    );

    let o = outline(&path, &options);
    // Curved join, with cubic control points around the corner.
    assert_contains_point(&o, point(102.761421, -5.0));
    assert_contains_point(&o, point(105.0, -2.761424));
    assert_contains_point(&o, point(105.0, 0.0));
    // End round cap control points and landing point.
    assert_contains_point(&o, point(105.0, 102.761421));
    assert_contains_point(&o, point(102.761421, 105.0));
    assert_contains_point(&o, point(100.0, 105.0));
}

#[test]
fn triangle_caps_bevel_joins_and_inverted_round_cap() {
    let path = l_path();
    let options = StrokeOptions::new(10.0)
        .with_line_join(LineJoin::Bevel)
        .with_line_cap(LineCap::Triangle)
        .with_tolerance(0.1);

    let o = outline(&path, &options);
    assert_contains_point(&o, point(-5.0, 0.0));
    assert_contains_point(&o, point(100.0, 105.0));
    assert_contains_point(&o, point(105.0, 0.0));

    // The inverted round cap carves the end back into the stroke while the
    // triangle-shaped join tip is preserved on the outer corner.
    let inverted = StrokeOptions::new(10.0)
        .with_end_cap(LineCap::RoundInverted)
        .with_tolerance(0.1);
    let o2 = outline(&path, &inverted);
    assert_contains_point(&o2, point(100.0, 95.0));
    assert_contains_point(&o2, point(105.0, 97.238579));
    assert_contains_point(&o2, point(102.761421, 95.0));
}

#[test]
fn asymmetric_offsets() {
    let mut b = Path::builder();
    b.begin(point(0.0, 0.0));
    b.line_to(point(40.0, 0.0));
    b.end(false);
    let path = b.build();

    let options = StrokeOptions::new(10.0)
        .with_offsets(-2.0, 8.0)
        .with_line_cap(LineCap::Butt)
        .with_tolerance(0.1);

    let o = outline(&path, &options);
    for corner in [
        point(0.0, -2.0),
        point(40.0, -2.0),
        point(40.0, 8.0),
        point(0.0, 8.0),
    ] {
        assert_contains_point(&o, corner);
    }
}

#[test]
fn cubic_curve() {
    let mut b = Path::builder();
    b.begin(point(0.0, 0.0));
    b.cubic_bezier_to(point(30.0, 80.0), point(90.0, -40.0), point(120.0, 10.0));
    b.end(false);
    let path = b.build();

    let options = StrokeOptions::new(10.0)
        .with_line_join(LineJoin::Miter)
        .with_miter_limit(4.0)
        .with_line_cap(LineCap::Butt)
        .with_tolerance(0.1);

    let m = mesh(&path, &options);
    assert_eq!((m.vertices.len(), m.indices.len()), (80, 234));
    let (a, b) = (m.vertices[78], m.vertices[79]);
    assert_near(a.position, point(-4.625709, 1.898108));
    assert!((a.normal.x + 0.925142).abs() < EPS);
    assert!((a.normal.y - 0.379622).abs() < EPS);
    assert_eq!(a.advancement, 0.0);
    assert_near(b.position, point(4.625709, -1.898108));

    // The contour is its own quadratic-bezier approximation, starting at a
    // different point than the mesh's flattened vertices.
    let o = outline(&path, &options);
    let first = match o.iter().next() {
        Some(lyon_path::PathEvent::Begin { at }) => at,
        other => panic!("expected a begin event, got {other:?}"),
    };
    assert_near(first, point(4.663379, -1.803578));
    assert!(
        o.iter()
            .any(|event| matches!(event, lyon_path::PathEvent::Quadratic { .. }))
    );
}

#[test]
fn filled_rectangle_stays_separate() {
    let mut b = Path::builder();
    b.begin(point(10.0, 20.0));
    b.line_to(point(70.0, 20.0));
    b.line_to(point(70.0, 50.0));
    b.line_to(point(10.0, 50.0));
    b.end(true);
    let path = b.build();

    // Fill tessellation is untouched by the stroke options unification.
    let mut buffers: VertexBuffers<Point, u16> = VertexBuffers::new();
    let mut tess = FillTessellator::new();
    tess.tessellate_path(
        &path,
        &FillOptions::default(),
        &mut simple_builder(&mut buffers),
    )
    .unwrap();

    assert_eq!((buffers.vertices.len(), buffers.indices.len()), (4, 6));
    assert_eq!(&buffers.indices[..], &[1, 0, 2, 1, 2, 3]);
    assert_near(buffers.vertices[0], point(10.0, 20.0));
    assert_near(buffers.vertices[1], point(70.0, 20.0));
    assert_near(buffers.vertices[2], point(10.0, 50.0));
    assert_near(buffers.vertices[3], point(70.0, 50.0));
}
