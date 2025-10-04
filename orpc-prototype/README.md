## ORPC and OQueue prototype

This project is a prototype for ORPC and OQueues. It is implemented entirely in Linux user-space and demonstrates the
framework and how it is used. The code here will hopefully be portable to kernel-space in a useful way, but it is
unlikely to be exactly as written here.

### Usage

The only real thing you can do is `cargo run` to run the demo. This should work on modern Linux. Other systems are
unknown and untested.

If you want to play with it and create some servers, you can try. Start at `prefetcher_demo` and modify what you find.
There is documentation on the framework (the macros and ORPC/OQueue library). Your editor should pick them up if you are
using rust-analyzer or an IDE. You can also generate the docs as HTML using `cargo doc` and look at them with your web
browser. There is no tutorial, yet.

Feel free to ask Arthur for help. Most of the knowledge needed for this will transfer to the actual Mariposa system, so
it will not be a waste of time.
