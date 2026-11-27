mod drive_data_purger;
mod drive_object_stores;
mod drive_purge_error;
mod noop_drive_data_purger;
mod s3_drive_data_purger;

pub use drive_data_purger::DriveDataPurger;
pub use drive_object_stores::{DriveObjectStores, EnvS3ObjectStores};
pub use drive_purge_error::DrivePurgeError;
pub use noop_drive_data_purger::NoopDriveDataPurger;
pub use s3_drive_data_purger::S3DriveDataPurger;
#[cfg(test)]
use s3_drive_data_purger::purge_drive;

#[cfg(test)]
mod tests;
