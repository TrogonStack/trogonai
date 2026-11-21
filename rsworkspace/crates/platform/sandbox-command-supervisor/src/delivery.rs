use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum Delivery {
    NotDelivered,
    Delivered,
}

impl From<bool> for Delivery {
    fn from(value: bool) -> Self {
        if value { Self::Delivered } else { Self::NotDelivered }
    }
}

impl From<Delivery> for bool {
    fn from(value: Delivery) -> Self {
        matches!(value, Delivery::Delivered)
    }
}
