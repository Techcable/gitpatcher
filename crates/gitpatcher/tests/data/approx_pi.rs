/// Compute the area of a polygon based on its apothem
#[inline]
fn polygon_apothem_area(apothem: f64, sides: u16) -> f64 {
    0.5 * (sides as f64) * apothem
}

fn main() {
    for sides in 0..16 {
        println!("{} sides: {}", sides, polygon_apothem_area(1.0, sides));
    }
}


