# Data OQueues available in OSTD

## Scheduler

Scheduling events can placed on an OQueue. Unlike most OQueues, this is disabled by default, because
of the potential for unknown overhead in a very sensitive part of the system. It can be enabled with
the `capture_scheduling` feature. You can enable this feature with `--features
ostd/capture_scheduling` on the `cargo osdk` command line. (Or the
`FEATURES=ostd/capture_scheduling` environment variable for OSTDs own makefiles.)

(NOTE: Once we are confident that the overhead is low enough, `capture_scheduling` will be enabled
by default.)

To have the the Mariposa kernel capture the scheduling events to a file, add this *and* the kernel
command line argument `scheduler.capture_data=true`.