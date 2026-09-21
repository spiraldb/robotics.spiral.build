//! Imported descriptions and their resolved meshes; no SO-101 conventions.
use crate::urdf::{Geometry, Urdf};
use glam::Vec3;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct RobotModel {
    pub(crate) description: Urdf,
    pub(crate) meshes: BTreeMap<String, Vec<f32>>,
    pub(crate) grid_height: f32,
}

impl RobotModel {
    pub fn parse(urdf: &str, assets: &BTreeMap<String, Vec<u8>>) -> Result<Self, String> {
        let description = Urdf::parse(urdf).map_err(|error| error.to_string())?;
        for link in description.links() {
            for visual in link.visuals() {
                let valid = match visual.geometry() {
                    Geometry::Box { size } => size.min_element() > 0.0,
                    Geometry::Cylinder { radius, length } => *radius > 0.0 && *length > 0.0,
                    Geometry::Sphere { radius } => *radius > 0.0,
                    Geometry::Mesh { scale, .. } => scale.abs().min_element() > 0.0,
                };
                if !valid {
                    return Err("Robot visual has a missing or invalid dimension".into());
                }
            }
        }
        let mut meshes = BTreeMap::new();
        for filename in Self::mesh_files(urdf)? {
            let bytes = assets
                .get(&filename)
                .ok_or_else(|| format!("Missing robot mesh: {filename}"))?;
            // Never let a supplied mesh read the server's filesystem.
            let scene = mesh_loader::Loader::default()
                .load_from_slice_with_reader(bytes, &filename, |_| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "External mesh material is unavailable",
                    ))
                })
                .map_err(|error| format!("Reading {filename}: {error}"))?;
            let mut vertices = Vec::new();
            for mesh in scene.meshes {
                for face in mesh.faces {
                    let points = face.map(|index| {
                        mesh.vertices
                            .get(index as usize)
                            .copied()
                            .map(Vec3::from_array)
                    });
                    let [Some(a), Some(b), Some(c)] = points else {
                        return Err("Invalid mesh face".into());
                    };
                    let normal = (b - a).cross(c - a).normalize_or(Vec3::Z);
                    for point in [a, b, c] {
                        if !point.is_finite() {
                            return Err("Non-finite mesh position".into());
                        }
                        vertices.extend(point.to_array());
                        vertices.extend(normal.to_array());
                    }
                }
            }
            if vertices.is_empty() {
                return Err(format!("Robot mesh {filename} has no triangles"));
            }
            meshes.insert(filename, vertices);
        }
        if description
            .links()
            .iter()
            .all(|link| link.visuals().is_empty())
        {
            return Err("Robot description has no visual geometry".into());
        }
        let mut model = Self {
            description,
            meshes,
            grid_height: 0.0,
        };
        model.grid_height = model.bounds(&BTreeMap::new()).0.z;
        Ok(model)
    }

    pub fn mesh_files(urdf: &str) -> Result<Vec<String>, String> {
        let description = Urdf::parse(urdf).map_err(|error| error.to_string())?;
        Ok(description
            .links()
            .iter()
            .flat_map(|link| link.visuals())
            .filter_map(|visual| match visual.geometry() {
                Geometry::Mesh { filename, .. } => Some(filename.clone()),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    pub fn tool_position(&self, link: &str, positions: &BTreeMap<String, f32>) -> Option<[f32; 3]> {
        let index = self.description.link_index(link)?;
        Some(
            (self.description.recorded_pose(positions)[index].transform_point3(Vec3::ZERO)
                * 1000.0)
                .to_array(),
        )
    }
    pub fn joint_fraction(&self, joint: &str, positions: &BTreeMap<String, f32>) -> Option<f32> {
        let (lower, upper) = self.description.joint(joint)?.limit()?;
        let position = positions.get(joint)?;
        (upper > lower).then(|| ((position - lower) / (upper - lower)).clamp(0.0, 1.0))
    }

    pub fn independent_joints(&self) -> impl Iterator<Item = &str> {
        self.description.independent_joints()
    }

    pub(crate) fn bounds(&self, positions: &BTreeMap<String, f32>) -> (Vec3, Vec3) {
        let transforms = self.description.recorded_pose(positions);
        let mut minimum = Vec3::splat(f32::INFINITY);
        let mut maximum = Vec3::splat(f32::NEG_INFINITY);
        for (visual, transform) in self.description.visuals(&transforms) {
            let extent = match visual.geometry() {
                Geometry::Box { size } => *size * 0.5,
                Geometry::Sphere { radius } => Vec3::splat(*radius),
                Geometry::Cylinder { radius, length } => Vec3::new(*radius, *radius, *length * 0.5),
                Geometry::Mesh { filename, scale } => {
                    for vertex in self.meshes[filename].as_chunks::<6>().0 {
                        let point = transform
                            .transform_point3(Vec3::new(vertex[0], vertex[1], vertex[2]) * *scale);
                        minimum = minimum.min(point);
                        maximum = maximum.max(point);
                    }
                    continue;
                }
            };
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        let point = transform.transform_point3(extent * Vec3::new(x, y, z));
                        minimum = minimum.min(point);
                        maximum = maximum.max(point);
                    }
                }
            }
        }
        (minimum, maximum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture;
    impl Fixture {
        fn gripper() -> String {
            r#"<robot name="gripper">
              <link name="base"><visual><geometry><box size="0.1 0.1 0.1"/></geometry></visual></link>
              <link name="left"/><link name="right"/>
              <joint name="opening" type="prismatic"><parent link="base"/><child link="left"/><axis xyz="0 1 0"/><limit lower="0" upper="0.04"/></joint>
              <joint name="following" type="prismatic"><parent link="base"/><child link="right"/><axis xyz="0 1 0"/><mimic joint="opening" multiplier="-1" offset="0.01"/></joint>
            </robot>"#.into()
        }
    }

    #[test]
    fn recorded_positions_use_model_units_without_live_offsets_or_clamping() {
        let model = RobotModel::parse(&Fixture::gripper(), &BTreeMap::new()).unwrap();
        assert_eq!(model.independent_joints().collect::<Vec<_>>(), ["opening"]);
        let poses = model
            .description
            .recorded_pose(&BTreeMap::from([("opening".into(), 0.08)]));
        let left =
            poses[model.description.link_index("left").unwrap()].transform_point3(Vec3::ZERO);
        let right =
            poses[model.description.link_index("right").unwrap()].transform_point3(Vec3::ZERO);
        assert!((left.y - 0.08).abs() < 1e-6);
        assert!((right.y + 0.07).abs() < 1e-6);
    }

    #[test]
    fn unusable_kinematics_are_rejected() {
        let model = Fixture::gripper();
        for invalid in [
            model.replace("joint=\"opening\"", "joint=\"following\""),
            model.replace("joint=\"opening\"", "joint=\"unknown\""),
            model.replace("prismatic", "floating"),
            model.replace("0 1 0", "0 NaN 0"),
            model.replace("name=\"following\"", "name=\"opening\""),
            model.replace("0.1 0.1 0.1", "0.1 -0.1 0.1"),
        ] {
            assert!(
                RobotModel::parse(&invalid, &BTreeMap::new()).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn supplied_meshes_are_required_and_resolve_scaled_geometry() {
        let urdf = r#"<robot name="mesh"><link name="base"><visual><origin xyz="5 0 1"/><geometry><mesh filename="arm.stl" scale="-2 2 2"/></geometry></visual></link></robot>"#;
        let stl = b"solid arm\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid arm\n";
        assert!(RobotModel::parse(urdf, &BTreeMap::new()).is_err());
        let model =
            RobotModel::parse(urdf, &BTreeMap::from([("arm.stl".into(), stl.to_vec())])).unwrap();
        assert_eq!(model.meshes["arm.stl"].len(), 18);
        assert_eq!(
            model.bounds(&BTreeMap::new()),
            (Vec3::new(3.0, 0.0, 1.0), Vec3::new(5.0, 2.0, 1.0))
        );
    }
}
