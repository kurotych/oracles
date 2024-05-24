// use hex_assignments::assignment::HexAssignments;
// use hex_assignments::HexAssignment;

pub mod data_sets;
pub use data_sets::*;

// #[derive(derive_builder::Builder)]
// #[builder(pattern = "owned")]
// pub struct HexBoostData<Foot, Land, Urban> {
//     pub footfall: Foot,
//     pub landtype: Land,
//     pub urbanization: Urban,
// }
// impl<F, L, U> HexBoostData<F, L, U> {
//     pub fn builder() -> HexBoostDataBuilder<F, L, U> {
//         HexBoostDataBuilder::default()
//     }
// }

// impl<Foot, Land, Urban> HexBoostData<Foot, Land, Urban>
// where
//     Foot: DataSet,
//     Land: DataSet,
//     Urban: DataSet,
// {
//     pub fn is_ready(&self) -> bool {
//         self.urbanization.is_ready() && self.footfall.is_ready() && self.landtype.is_ready()
//     }
// }

// impl<Foot, Land, Urban> HexBoostData<Foot, Land, Urban>
// where
//     Foot: HexAssignment,
//     Land: HexAssignment,
//     Urban: HexAssignment,
// {
//     pub fn assignments(&self, cell: hextree::Cell) -> anyhow::Result<HexAssignments> {
//         HexAssignments::builder(cell)
//             .footfall(&self.footfall)
//             .landtype(&self.landtype)
//             .urbanized(&self.urbanization)
//             .build()
//     }
// }
