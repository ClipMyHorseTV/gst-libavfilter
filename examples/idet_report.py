#!/usr/bin/env python3
"""Print avfilter_idet's per-frame verdicts.

Usage: GST_PLUGIN_PATH=target/debug ./examples/idet_report.py video.ts
"""
import sys

import gi

gi.require_version("Gst", "1.0")
from gi.repository import GLib, Gst  # noqa: E402

Gst.init(None)
pipeline = Gst.parse_launch(
    f"filesrc location={sys.argv[1]} ! decodebin ! videoconvert ! video/x-raw,format=I420 "
    "! avfilter_idet ! fakesink sync=false"
)
loop = GLib.MainLoop()


def on_message(bus, msg):
    if msg.type == Gst.MessageType.ELEMENT and msg.get_structure().get_name() == "avfilter-idet":
        s = msg.get_structure()
        ok, ts = s.get_uint64("timestamp")
        print(
            f"{ts / Gst.SECOND if ok else -1:8.3f}s  single={s.get_string('single'):<13}"
            f"multiple={s.get_string('multiple'):<13}repeated={s.get_string('repeated')}"
        )
    elif msg.type == Gst.MessageType.ERROR:
        print(msg.parse_error())
        loop.quit()
    elif msg.type == Gst.MessageType.EOS:
        loop.quit()


bus = pipeline.get_bus()
bus.add_signal_watch()
bus.connect("message", on_message)
pipeline.set_state(Gst.State.PLAYING)
loop.run()
pipeline.set_state(Gst.State.NULL)
