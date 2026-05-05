#[path="delivery.u.pb.rs"]
#[allow(nonstandard_style)]
pub mod internal_do_not_use_trogon_scron_sjobs_sv1_sdelivery;

#[allow(unused_imports, nonstandard_style)]
pub use internal_do_not_use_trogon_scron_sjobs_sv1_sdelivery::*;
#[path="message.u.pb.rs"]
#[allow(nonstandard_style)]
pub mod internal_do_not_use_trogon_scron_sjobs_sv1_smessage;

#[allow(unused_imports, nonstandard_style)]
pub use internal_do_not_use_trogon_scron_sjobs_sv1_smessage::*;
#[path="schedule.u.pb.rs"]
#[allow(nonstandard_style)]
pub mod internal_do_not_use_trogon_scron_sjobs_sv1_sschedule;

#[allow(unused_imports, nonstandard_style)]
pub use internal_do_not_use_trogon_scron_sjobs_sv1_sschedule::*;
#[path="job_details.u.pb.rs"]
#[allow(nonstandard_style)]
pub mod internal_do_not_use_trogon_scron_sjobs_sv1_sjob__details;

#[allow(unused_imports, nonstandard_style)]
pub use internal_do_not_use_trogon_scron_sjobs_sv1_sjob__details::*;
#[path="job_added.u.pb.rs"]
#[allow(nonstandard_style)]
pub mod internal_do_not_use_trogon_scron_sjobs_sv1_sjob__added;

#[allow(unused_imports, nonstandard_style)]
pub use internal_do_not_use_trogon_scron_sjobs_sv1_sjob__added::*;
#[path="job_paused.u.pb.rs"]
#[allow(nonstandard_style)]
pub mod internal_do_not_use_trogon_scron_sjobs_sv1_sjob__paused;

#[allow(unused_imports, nonstandard_style)]
pub use internal_do_not_use_trogon_scron_sjobs_sv1_sjob__paused::*;
#[path="job_removed.u.pb.rs"]
#[allow(nonstandard_style)]
pub mod internal_do_not_use_trogon_scron_sjobs_sv1_sjob__removed;

#[allow(unused_imports, nonstandard_style)]
pub use internal_do_not_use_trogon_scron_sjobs_sv1_sjob__removed::*;
#[path="job_resumed.u.pb.rs"]
#[allow(nonstandard_style)]
pub mod internal_do_not_use_trogon_scron_sjobs_sv1_sjob__resumed;

#[allow(unused_imports, nonstandard_style)]
pub use internal_do_not_use_trogon_scron_sjobs_sv1_sjob__resumed::*;
#[path="events.u.pb.rs"]
#[allow(nonstandard_style)]
pub mod internal_do_not_use_trogon_scron_sjobs_sv1_sevents;

#[allow(unused_imports, nonstandard_style)]
pub use internal_do_not_use_trogon_scron_sjobs_sv1_sevents::*;
pub mod __unstable {
pub static TROGON_CRON_JOBS_V1_DELIVERY_DESCRIPTOR_INFO: ::protobuf::__internal::runtime::__unstable::DescriptorInfo = ::protobuf::__internal::runtime::__unstable::DescriptorInfo {
  descriptor: b"\n\"trogon/cron/jobs/v1/delivery.proto\x12\x13trogon.cron.jobs.v1\"^\n\x0bJobDelivery\x12G\n\nnats_event\x18\x01 \x01(\x0b\x32&.trogon.cron.jobs.v1.NatsEventDeliveryH\x00R\tnatsEventB\x06\n\x04kind\"\x82\x01\n\x11NatsEventDelivery\x12\x14\n\x05route\x18\x01 \x01(\tR\x05route\x12\x17\n\x07ttl_sec\x18\x02 \x01(\x04R\x06ttlSec\x12>\n\x06source\x18\x03 \x01(\x0b\x32&.trogon.cron.jobs.v1.JobSamplingSourceR\x06source\"}\n\x11JobSamplingSource\x12`\n\x13latest_from_subject\x18\x01 \x01(\x0b\x32..trogon.cron.jobs.v1.LatestFromSubjectSamplingH\x00R\x11latestFromSubjectB\x06\n\x04kind\"5\n\x19LatestFromSubjectSampling\x12\x18\n\x07subject\x18\x01 \x01(\tR\x07subjectb\x08\x65\x64itionsp\xe9\x07",
  deps: &[
  ],
};
pub static TROGON_CRON_JOBS_V1_MESSAGE_DESCRIPTOR_INFO: ::protobuf::__internal::runtime::__unstable::DescriptorInfo = ::protobuf::__internal::runtime::__unstable::DescriptorInfo {
  descriptor: b"\n!trogon/cron/jobs/v1/message.proto\x12\x13trogon.cron.jobs.v1\"]\n\nJobMessage\x12\x18\n\x07\x63ontent\x18\x01 \x01(\tR\x07\x63ontent\x12\x35\n\x07headers\x18\x02 \x03(\x0b\x32\x1b.trogon.cron.jobs.v1.HeaderR\x07headers\"2\n\x06Header\x12\x12\n\x04name\x18\x01 \x01(\tR\x04name\x12\x14\n\x05value\x18\x02 \x01(\tR\x05valueb\x08\x65\x64itionsp\xe9\x07",
  deps: &[
  ],
};
pub static TROGON_CRON_JOBS_V1_SCHEDULE_DESCRIPTOR_INFO: ::protobuf::__internal::runtime::__unstable::DescriptorInfo = ::protobuf::__internal::runtime::__unstable::DescriptorInfo {
  descriptor: b"\n\"trogon/cron/jobs/v1/schedule.proto\x12\x13trogon.cron.jobs.v1\"\xbd\x01\n\x0bJobSchedule\x12\x31\n\x02\x61t\x18\x01 \x01(\x0b\x32\x1f.trogon.cron.jobs.v1.AtScheduleH\x00R\x02\x61t\x12:\n\x05\x65very\x18\x02 \x01(\x0b\x32\".trogon.cron.jobs.v1.EveryScheduleH\x00R\x05\x65very\x12\x37\n\x04\x63ron\x18\x03 \x01(\x0b\x32!.trogon.cron.jobs.v1.CronScheduleH\x00R\x04\x63ronB\x06\n\x04kind\"\x1c\n\nAtSchedule\x12\x0e\n\x02\x61t\x18\x01 \x01(\tR\x02\x61t\",\n\rEverySchedule\x12\x1b\n\tevery_sec\x18\x01 \x01(\x04R\x08\x65verySec\">\n\x0c\x43ronSchedule\x12\x12\n\x04\x65xpr\x18\x01 \x01(\tR\x04\x65xpr\x12\x1a\n\x08timezone\x18\x02 \x01(\tR\x08timezoneb\x08\x65\x64itionsp\xe9\x07",
  deps: &[
  ],
};
pub static TROGON_CRON_JOBS_V1_JOB_DETAILS_DESCRIPTOR_INFO: ::protobuf::__internal::runtime::__unstable::DescriptorInfo = ::protobuf::__internal::runtime::__unstable::DescriptorInfo {
  descriptor: b"\n%trogon/cron/jobs/v1/job_details.proto\x12\x13trogon.cron.jobs.v1\x1a\"trogon/cron/jobs/v1/delivery.proto\x1a!trogon/cron/jobs/v1/message.proto\x1a\"trogon/cron/jobs/v1/schedule.proto\"\xfb\x01\n\nJobDetails\x12\x36\n\x06status\x18\x01 \x01(\x0e\x32\x1e.trogon.cron.jobs.v1.JobStatusR\x06status\x12<\n\x08schedule\x18\x02 \x01(\x0b\x32 .trogon.cron.jobs.v1.JobScheduleR\x08schedule\x12<\n\x08\x64\x65livery\x18\x03 \x01(\x0b\x32 .trogon.cron.jobs.v1.JobDeliveryR\x08\x64\x65livery\x12\x39\n\x07message\x18\x04 \x01(\x0b\x32\x1f.trogon.cron.jobs.v1.JobMessageR\x07message*X\n\tJobStatus\x12\x1a\n\x16JOB_STATUS_UNSPECIFIED\x10\x00\x12\x16\n\x12JOB_STATUS_ENABLED\x10\x01\x12\x17\n\x13JOB_STATUS_DISABLED\x10\x02\x62\x08\x65\x64itionsp\xe9\x07",
  deps: &[
    &super::__unstable::TROGON_CRON_JOBS_V1_DELIVERY_DESCRIPTOR_INFO,
    &super::__unstable::TROGON_CRON_JOBS_V1_MESSAGE_DESCRIPTOR_INFO,
    &super::__unstable::TROGON_CRON_JOBS_V1_SCHEDULE_DESCRIPTOR_INFO,
  ],
};
pub static TROGON_CRON_JOBS_V1_JOB_ADDED_DESCRIPTOR_INFO: ::protobuf::__internal::runtime::__unstable::DescriptorInfo = ::protobuf::__internal::runtime::__unstable::DescriptorInfo {
  descriptor: b"\n#trogon/cron/jobs/v1/job_added.proto\x12\x13trogon.cron.jobs.v1\x1a%trogon/cron/jobs/v1/job_details.proto\"=\n\x08JobAdded\x12\x31\n\x03job\x18\x01 \x01(\x0b\x32\x1f.trogon.cron.jobs.v1.JobDetailsR\x03jobb\x08\x65\x64itionsp\xe9\x07",
  deps: &[
    &super::__unstable::TROGON_CRON_JOBS_V1_JOB_DETAILS_DESCRIPTOR_INFO,
  ],
};
pub static TROGON_CRON_JOBS_V1_JOB_PAUSED_DESCRIPTOR_INFO: ::protobuf::__internal::runtime::__unstable::DescriptorInfo = ::protobuf::__internal::runtime::__unstable::DescriptorInfo {
  descriptor: b"\n$trogon/cron/jobs/v1/job_paused.proto\x12\x13trogon.cron.jobs.v1\"\x0b\n\tJobPausedb\x08\x65\x64itionsp\xe9\x07",
  deps: &[
  ],
};
pub static TROGON_CRON_JOBS_V1_JOB_REMOVED_DESCRIPTOR_INFO: ::protobuf::__internal::runtime::__unstable::DescriptorInfo = ::protobuf::__internal::runtime::__unstable::DescriptorInfo {
  descriptor: b"\n%trogon/cron/jobs/v1/job_removed.proto\x12\x13trogon.cron.jobs.v1\"\x0c\n\nJobRemovedb\x08\x65\x64itionsp\xe9\x07",
  deps: &[
  ],
};
pub static TROGON_CRON_JOBS_V1_JOB_RESUMED_DESCRIPTOR_INFO: ::protobuf::__internal::runtime::__unstable::DescriptorInfo = ::protobuf::__internal::runtime::__unstable::DescriptorInfo {
  descriptor: b"\n%trogon/cron/jobs/v1/job_resumed.proto\x12\x13trogon.cron.jobs.v1\"\x0c\n\nJobResumedb\x08\x65\x64itionsp\xe9\x07",
  deps: &[
  ],
};
pub static TROGON_CRON_JOBS_V1_EVENTS_DESCRIPTOR_INFO: ::protobuf::__internal::runtime::__unstable::DescriptorInfo = ::protobuf::__internal::runtime::__unstable::DescriptorInfo {
  descriptor: b"\n trogon/cron/jobs/v1/events.proto\x12\x13trogon.cron.jobs.v1\x1a#trogon/cron/jobs/v1/job_added.proto\x1a$trogon/cron/jobs/v1/job_paused.proto\x1a%trogon/cron/jobs/v1/job_removed.proto\x1a%trogon/cron/jobs/v1/job_resumed.proto\"\x9a\x02\n\x08JobEvent\x12<\n\tjob_added\x18\x01 \x01(\x0b\x32\x1d.trogon.cron.jobs.v1.JobAddedH\x00R\x08jobAdded\x12?\n\njob_paused\x18\x02 \x01(\x0b\x32\x1e.trogon.cron.jobs.v1.JobPausedH\x00R\tjobPaused\x12\x42\n\x0bjob_resumed\x18\x03 \x01(\x0b\x32\x1f.trogon.cron.jobs.v1.JobResumedH\x00R\njobResumed\x12\x42\n\x0bjob_removed\x18\x04 \x01(\x0b\x32\x1f.trogon.cron.jobs.v1.JobRemovedH\x00R\njobRemovedB\x07\n\x05\x65ventb\x08\x65\x64itionsp\xe9\x07",
  deps: &[
    &super::__unstable::TROGON_CRON_JOBS_V1_JOB_ADDED_DESCRIPTOR_INFO,
    &super::__unstable::TROGON_CRON_JOBS_V1_JOB_PAUSED_DESCRIPTOR_INFO,
    &super::__unstable::TROGON_CRON_JOBS_V1_JOB_REMOVED_DESCRIPTOR_INFO,
    &super::__unstable::TROGON_CRON_JOBS_V1_JOB_RESUMED_DESCRIPTOR_INFO,
  ],
};
}
