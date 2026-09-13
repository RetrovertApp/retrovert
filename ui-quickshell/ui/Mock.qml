pragma Singleton

import QtQuick

// Placeholder library content from the design, until the catalog and playlist crates feed the
// shell. Every list here is what ui-ref/retrovert-2a-library.png shows.
QtObject {
    readonly property var formats: [
        { label: "All formats", count: "4,812" }, { label: "Amiga MOD", count: "1,406" },
        { label: "XM · FT2", count: "1,122" }, { label: "IT · S3M", count: "684" },
        { label: "SID · C64", count: "912" }, { label: "SPC · VGM · NSF", count: "571" },
        { label: "AHX · YM · other", count: "117" }
    ]
    readonly property var collections: [
        { label: "Party releases 2025", count: "84" }, { label: "Keygen bangers", count: "132" },
        { label: "Game rips", count: "640" }, { label: "Modland mirror", count: "2,911" }
    ]
    readonly property var modules: [
        { title: "Neon Overdrive", composer: "Kaleido", group: "Rift Crew", fmt: "XM", ch: 8, year: 1997, dur: "3:48" },
        { title: "Copper Bars Forever", composer: "mnemo", group: "Voxelheart", fmt: "MOD", ch: 4, year: 1992, dur: "2:56" },
        { title: "Phosphor Dreams", composer: "Tesseract", group: "Pulsewidth", fmt: "IT", ch: 16, year: 1999, dur: "4:12" },
        { title: "Kernel Panic", composer: "Zirkon", group: "Static Union", fmt: "XM", ch: 12, year: 2001, dur: "3:31" },
        { title: "Raster Interrupt", composer: "Laserdanz", group: "Cathode Kids", fmt: "SID", ch: 3, year: 1989, dur: "5:10" },
        { title: "Sprite Multiplexer", composer: "Ophelia B.", group: "Voxelheart", fmt: "SID", ch: 3, year: 1991, dur: "3:19" },
        { title: "Blast Processing", composer: "Kaleido", group: "Rift Crew", fmt: "VGM", ch: 6, year: 1994, dur: "2:44" },
        { title: "Mode 7 Sunset", composer: "Tesseract", group: "—", fmt: "SPC", ch: 8, year: 1995, dur: "4:01" },
        { title: "Hardsync", composer: "Laserdanz", group: "Cathode Kids", fmt: "AHX", ch: 4, year: 1996, dur: "3:07" },
        { title: "Bitplane Blues", composer: "mnemo", group: "Voxelheart", fmt: "MOD", ch: 4, year: 1993, dur: "3:55" },
        { title: "Scanline Lovers", composer: "Zirkon", group: "Static Union", fmt: "XM", ch: 8, year: 2003, dur: "4:28" },
        { title: "Fastloader", composer: "Ophelia B.", group: "Cathode Kids", fmt: "SID", ch: 3, year: 1990, dur: "2:48" },
        { title: "Wavetable Rain", composer: "Tesseract", group: "Pulsewidth", fmt: "IT", ch: 24, year: 2004, dur: "6:03" },
        { title: "Trainer Menu", composer: "Kaleido", group: "Rift Crew", fmt: "MOD", ch: 4, year: 1991, dur: "1:52" },
        { title: "Attract Mode", composer: "Zirkon", group: "—", fmt: "NSF", ch: 5, year: 1988, dur: "2:20" },
        { title: "Party Coder Anthem", composer: "mnemo", group: "Voxelheart", fmt: "XM", ch: 10, year: 1998, dur: "4:36" }
    ]
    readonly property int playingIndex: 2
    readonly property var queue: [3, 4, 6, 7, 8, 10, 12].map(function (i) { return modules[i] })
    readonly property var samples: [
        "01 kick_909_tight", "02 snare_reverse", "03 hat_closed", "04 bass_saw_dt", "05 pad_glass_c4",
        "06 lead_pwm", "07 choir_ahh_c3", "08 arp_square", "09 crash_long", "10 tom_lo", "11 tom_hi",
        "12 clap_layer", "13 string_ens", "14 fx_riser", "15 bell_fm", "16 noise_wash"
    ]
    readonly property var vu: [0.9, 0.5, 0.08, 0.7, 0.08, 0.6, 0.8, 0.08, 0.4, 0.08, 0.95, 0.5, 0.08, 0.65, 0.08, 0.35]
}
