include!("../../contracts/build_support.rs");

fn main() {
    generate_boundary_constants(BoundaryTarget::TrainingApi);
}
