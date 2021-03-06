#[feature(struct_variant)];
#[link(name = "midi",
       vers = "0.1",
       package_id = "bd61bdf8938c3e8a50c7105b949065a1")];
/// Package ID is name concatenated with vers, separated by space, fed to md5sum

/** A library providing functions to read and write files in the MIDI file format. Most of this was
 * taken from the high-level view provided at 
 *
 * http://faydoc.tripod.com/formats/mid.htm
 *
 * and the more comprehensive set of pages at 
 *
 * http://www.recordingblogs.com/sa/tabid/88/Default.aspx?topic=Musical+Instrument+Digital+Interface+(MIDI)
 *
 * The official MIDI spec is here:
 *
 * http://www.midi.org/techspecs/midispec.php
 */

#[crate_type = "lib"]

#[desc = "MIDI library for rust. Provides programmatic access to data in MIDI files."]
#[license = "GPL"]
#[author = "Paul Meier"]

#[warn(non_camel_case_types)]

use std::io::{File, io_error};
use std::option::{Some, None};
use std::path::Path;
use std::vec::{with_capacity, append_one};

// TODO:  Write a Rust macro to chain Option<> Pattern matches, so Nones always just return None,
// but assume you got the Some(x)?
// TODO: Parallelize the track reads rather than make it sequential?


// Reading
/// A MidiFile contains a Header, and a list of Tracks,
pub struct MidiFile {
    header: MidiHeader,
    tracks: ~[MidiTrack]
}

/// A MidiHeader contains the FileFormat, the number of tracks, and the 'ticks' per quarter note.
pub struct MidiHeader {
    file_format: FileFormat,
    num_tracks: u16,
    ticks_per_quarter: u16
}

/// A Miditrack itself only contains its own length and a list of the events.
pub struct MidiTrack {
    track_length: u32,
    events: ~[MidiEvent]
}

pub struct MidiEvent {
    delta_time: u32,
    message: MidiMessage
}

/// MIDI files can have one of three formats, defined in the header of the file.
pub enum FileFormat {
    SingleTrack = 1,
    MultipleSynchronous = 2,
    MultipleAsynchronous = 3
}

/// Not public, this is the contextual data necessary to read a track -- some cases like "running
/// mode" mean that you need to repeat the previous status. We store the contextual data here.
struct ContinueTrackRead {
    offset : u32,
    last_status : u8
}

/// The various commands a MidiMessage can contain. Codes and descriptions lifted from 
/// http://www.recordingblogs.com/sa/tabid/88/Default.aspx?topic=Status+byte+(of+a+MIDI+message)
pub enum MidiMessage {
    /// Release a note and stop playing it.
    NoteOff { channel: u8, key : u8, velocity : u8},
    /// Play a note and start sounding it.
    NoteOn { channel: u8, key : u8, velocity : u8},
    /// Apply pressure to a note playing, similar to applying pressure to electronic keyboard keys.
    Aftertouch { channel: u8, key : u8, velocity : u8},
    /// Affect a controller, such as a slider, knob, or switch.
    ControlChange { channel: u8, controller : u8, value : u8},
    /// Assign a program to a MIDI channel, such as an instrument, patch, or preset.
    ProgramChange { channel : u8, new_program : u8 },
    /// Apply pressure to a MIDI channel, similar to applying pressure to electronic keyboard keys.
    ChannelPressure { channel : u8, value : u8 },
    /// Change a channel pitch up or down.
    PitchWheel { channel : u8, lsb : u8, msb : u8 },

    /// Perform some device specific task.
    SystemExclusive { amei : u32, nope : u8 },
    /// Set the MIDI time to keep in line with some other device.
    MidiTimeCode { message_type : u8, values : u8 },
    /// Cue to a point in the MIDI sequence to be ready to play.
    SongPositionPointer { lsb : u8, msb : u8 },
    /// Set a sequence for playback.
    SongSelect { song : u8 },
    /// Tune.
    TuneRequest,
    /// Understand the position of the MIDI clock (when synchronized to another device).
    MidiClock,
    /// Start playback of some MIDI sequence.
    MidiStart,
    /// Resume playback of some MIDI sequence.
    MidiContinue,
    /// Stop playback of some MIDI sequence.
    MidiStop,
    /// Understand that a MIDI connection exists (if there are no other MIDI messages).
    ActiveSense,
    /// Reset to default state.
    Reset,
    /// Not a valid status, repeat previous message, per "running mode," where you can omit a status.
    InvalidStatus
}

pub fn parse_file(filename : &str) -> Option<MidiFile> {
    // Open the file according to the filename
    let path = &Path::new(filename);

    do io_error::cond.trap(|_| {
        // error on file IO
        error!("Issue with file!");
    }).inside {
        let contents_buf = File::open(path).read_to_end();
        match parse_header(contents_buf) {
            Some(header) => {
                match parse_all_tracks(header, contents_buf) {
                    Some(tracks) => {
                        let new_midifile = MidiFile{header: header, tracks : tracks};
                        Some(new_midifile)
                    }
                    None => { None }
                } // match parse_all_tracks
            }
            None => { None }
        } // match parse_header
    }
}


/// Parses the first 14 bytes, which comprise a MIDI header.
fn parse_header(buf : &[u8]) -> Option<MidiHeader> {
    let err = buf[0] != ('M' as u8) || buf[1] != ('T' as u8)
           || buf[2] != ('h' as u8) || buf[3] != ('d' as u8)
           || buf[4] != 0           || buf[5] != 0
           || buf[6] != 0           || buf[7] != 6;

    if err {
        error!("Malformed MIDI header -- first 8 bytes nonstandard.");
        None
    } else {
        let ff = u16_from_u8_at(buf, 8);
        let num_tracks = u16_from_u8_at(buf, 10);
        let ticks_per_quarter = u16_from_u8_at(buf, 12);

        match file_format_from_u16(ff) {
            Some(x) => { Some(MidiHeader{file_format : x,
                                         num_tracks : num_tracks,
                                         ticks_per_quarter : ticks_per_quarter}) }
            None => {
                error!("Invalid file format in header.");
                None
            }
        }
    }
}

/// Parses all the tracks in a MIDI file, read into a buffer.
// TODO: This is a good candidate for parallel calls, rather than sequential.
fn parse_all_tracks(header : MidiHeader, buf : &[u8]) -> Option<~[MidiTrack]> {
    // Since the header is always constant size, we begin from 14.
    let mut offset = 14;
    let mut return_vec = with_capacity(header.num_tracks as uint);
    let mut error = false;

    for _ in range(0, header.num_tracks) {
        match parse_track(buf, offset) {
            Some(track) => {
                let length = track.track_length;
                return_vec = append_one(return_vec, track);
                // the '8' is for the header. Make a constant at top-level?
                offset += (length + 8);
            }
            None => {
                error = true;
            }
        }
    }
    if error {
        None
    } else {
        Some(return_vec)
    }
}

/// Parses an individual track beginning at the specified offset.
fn parse_track(buf : &[u8], offset : u32) -> Option<MidiTrack> {
    // chunk ID (4 bytes of MTrk)
    let err = buf[0] != ('M' as u8) || buf[1] != ('T' as u8)
           || buf[2] != ('r' as u8) || buf[3] != ('k' as u8);
    if err {
        error!("Malformed MIDI header -- first 4 bytes nonstandard, offset is {}", offset);
        None
    } else {
        let track_size = get_track_size(buf, offset);
        let event_offset = offset + 8;
        let mut midi_events = with_capacity(0);
        let mut error = false;
        let mut cont = ContinueTrackRead { offset : event_offset, last_status : 0x00 };
        // Parse events in sequence.
        while cont.offset < (event_offset + track_size) {
            match parse_event(buf, cont) {
                None => {
                    error!("Malformed event, somewhere near offset {}", event_offset);
                    error = true;
                    break;
                }
                Some((x, new_cont)) => {
                    midi_events = append_one(midi_events, x);
                    cont = new_cont;
                }
            }
        }
        match error {
            false => Some(MidiTrack{ track_length : track_size, events : midi_events }),
            true => None
        }
    }
}

fn parse_event(buf : &[u8], cont : ContinueTrackRead) -> Option<(MidiEvent, ContinueTrackRead)> {
    match parse_ticks(buf, cont.offset) {
        (ticks, new_offset) => {
           match parse_message(buf, new_offset, cont.last_status) {
               None => { None }
               Some((message, new_offset)) => { 
                    Some((MidiEvent{ delta_time : ticks, message : message },
                         ContinueTrackRead{ offset : new_offset, last_status : get_status_byte(message) }))
               }
           }
        }
    }
}

// MIDI spec says length should be at most 4 bytes, so some hardcoded values here. Should probably
// have more safety bits than the simple assert.
// 
// A small reminder of how MIDI Events work: you start with the number of ticks, followed by a MIDI
// message. This function parses the ticks, which is variable length.
//
// The number of ticks can be expressed with at least 1 and at most 4 bytes. All bytes must have a
// '1' in the highest order position, except the last, which must have a 0. When you've read all the
// bytes, you combine the bottom 7 of all of them into one long bitstring, then evaluate it for the
// number of ticks.
//
// The code is a little hairy since making a bunch of 8-bit bytes into 7-bit bytes to combine to
// some bitstring that is a multiple of 7... maybe my bitflip-foo isn't so good, but it's hard to
// find a way to do it easily and 'elegantly.' Instead, I store them all into 32-bit values, and
// once it's established how many there are (from time_offset) I combine them together by
// bitshift + OR.
fn parse_ticks(buf : &[u8], offset : u32) -> (u32, u32) {
    let mut time_offset = 0;
    let mut time_buffer : [u32, ..4] = [0,0,0,0];
    let mut return_value;
    loop {
        assert!(time_offset < 4);
        let curr = buf[offset + time_offset];
        time_buffer[time_offset] = (lower_seven_bits(curr) as u32);
        if msb_is_one(curr) {
            time_offset += 1;
        } else {
            let mut loop_offset = 0;
            let mut time_ticks = 0;
            while loop_offset <= time_offset {
                let byte_correction = time_offset - loop_offset;
                let contribution = (time_buffer[0 + loop_offset] << (8 * byte_correction)) >> byte_correction;
                time_ticks = time_ticks | contribution;
                loop_offset += 1;
            }
            return_value = (time_ticks, offset + time_offset + 1);
            break;
        }
    }
    return_value
}

fn parse_message(buf : &[u8], start_offset : u32, last_status : u8) -> Option<(MidiMessage, u32)> {

    let mut status_byte;
    let mut data_offset;
    if is_invalid_status_byte(buf[start_offset]) {
        status_byte = last_status;
        data_offset = start_offset;
    } else {
        status_byte = buf[start_offset];
        data_offset = start_offset + 1;
    }

    let status_pattern = status_byte & 0xF0;
    let channel_number = status_byte & 0x0F;
    match status_pattern {
        0x80 => {
            let k = lower_seven_bits(buf[data_offset]);
            let v = lower_seven_bits(buf[data_offset + 1]);
            Some((NoteOff{ channel : channel_number, key : k, velocity : v }, data_offset + 2))
        }
        0x90 => {
            let k = lower_seven_bits(buf[data_offset]);
            let v = lower_seven_bits(buf[data_offset + 1]);
            Some((NoteOn{ channel : channel_number, key : k, velocity : v }, data_offset + 2))
        }
        0xA0 => {
            let k = lower_seven_bits(buf[data_offset]);
            let v = lower_seven_bits(buf[data_offset + 1]);
            Some((Aftertouch{ channel : channel_number, key : k, velocity : v }, data_offset + 2))
        }
        0xB0 => {
            let c = lower_seven_bits(buf[data_offset]);
            let v = lower_seven_bits(buf[data_offset + 1]);
            Some((ControlChange{ channel : channel_number, controller : c, value : v }, data_offset + 2))
        }
        0xC0 => {
            let p = lower_seven_bits(buf[data_offset]);
            Some((ProgramChange{ channel : channel_number, new_program : p }, data_offset + 1))
        }
        0xD0 => {
            let v = lower_seven_bits(buf[data_offset]);
            Some((ChannelPressure{ channel : channel_number, value : v }, data_offset + 1))
        }
        0xE0 => {
            let l = lower_seven_bits(buf[data_offset]);
            let m = lower_seven_bits(buf[data_offset + 1]);
            Some((PitchWheel{ channel : channel_number, lsb : l, msb : m }, data_offset + 2))
        }
        0xF0 => {
            match channel_number {
                0x00 => {
                    // We don't support MIDI with system exclusive commands, can't even parse it
                    // since you don't know whether the AMEI is one or three bytes, nor do you know
                    // the length of what follows.
                    None
                }
                0x01 => {
                    let mt = lower_seven_bits(buf[data_offset]);
                    let v = lower_seven_bits(buf[data_offset + 1]);
                    Some((MidiTimeCode{ message_type : mt, values : v }, data_offset + 2))
                }
                0x02 => {
                    let l = lower_seven_bits(buf[data_offset]);
                    let m = lower_seven_bits(buf[data_offset + 1]);
                    Some((SongPositionPointer{ lsb : l, msb : m }, data_offset + 2))
                }
                0x03 => {
                    let s = lower_seven_bits(buf[data_offset]);
                    Some((SongSelect{ song : s }, data_offset + 1))
                }
                0x06 => {
                    Some((TuneRequest, data_offset))
                }
                0x08 => {
                    Some((MidiClock, data_offset))
                }
                0x0A => {
                    Some((MidiStart, data_offset))
                }
                0x0B => {
                    Some((MidiContinue, data_offset))
                }
                0x0C => {
                    Some((MidiStop, data_offset))
                }
                0x0E => {
                    Some((ActiveSense, data_offset))
                }
                0x0F => {
                    Some((Reset, data_offset))
                }
                _ => { None }
            }
        }
        _ => {
            Some((InvalidStatus, data_offset))
        }
    }
}

// Pretty-print
pub fn pretty_print(file : MidiFile) {
    println!("----- MIDI FILE -----");
    println!("*****\nHeader:");

    let format = file.header.file_format;
    let num_tracks = file.header.num_tracks;
    let tpq = file.header.ticks_per_quarter;

    println!("  File format: {}", file_format_to_string(format));
    println!("  Number of tracks: {}", num_tracks);
    println!("  Ticks per quarter note: {}", tpq);

    println!("*****\nTracks:");

    let mut track_number = 1;
    for track in file.tracks.iter() {
        println!("  Track {}", track_number);
        println!("  Track length: {}", track.track_length);
        for event in track.events.iter() {
            println!("    --");
            println!("    Delta time: {}", event.delta_time); 
            println!("    Message: {}", message_to_string(event.message));
        }
        track_number += 1;
    }
    println!("---------------------");
}

fn file_format_to_string(f : FileFormat) -> ~str {
    match f {
        SingleTrack => format!("Single Track"),
        MultipleSynchronous => format!("Multiple track, asynchronous"),
        MultipleAsynchronous => format!("Multiple track, synchronous")
    }
}

fn message_to_string(m : MidiMessage) -> ~str {
    match m {
        NoteOff         { channel : c, key : k, velocity : v } => { format!("NoteOff -- channel: {}, key: {}, velocity: {}", c, k, v) }
        NoteOn          { channel : c, key : k, velocity : v } => { format!("NoteOn -- channel: {}, key: {}, velocity: {}", c, k, v) }
        Aftertouch      { channel : c, key : k, velocity : v } => { format!("Aftertouch -- channel: {}, key: {}, velocity: {}", c, k, v) }
        ControlChange   { channel : ch, controller : c, value : v } => { format!("ControlChange -- channel: {}, controller : {}, value: {}", ch, c, v) }
        ProgramChange   { channel : c, new_program : p } => { format!("ProgramChange -- channel: {}, new_program: {}", c, p) }
        ChannelPressure { channel : c, value : v } => { format!("ChannelPressure -- channel: {}, value: {}", c, v) }
        PitchWheel      { channel : c,  lsb : l, msb : m } => { format!("PitchWheel -- channel: {}, lsb: {}, msb: {}", c, l, m) }

        SystemExclusive     {_} => { format!("SystemExclusive") }
        MidiTimeCode        {_} => { format!("MidiTimeCode") }
        SongPositionPointer {_} => { format!("SongPositionPointer") }
        SongSelect          {_} => { format!("SongSelect") }
        TuneRequest             => { format!("Tune Request") }
        MidiClock               => { format!("Midi Clock") }
        MidiStart               => { format!("Midi Start") }
        MidiContinue            => { format!("Midi Continue") }
        MidiStop                => { format!("Midi Stop") }
        ActiveSense             => { format!("Active Sense") }
        Reset                   => { format!("Reset") }
        // InvalidStatus gets an invalid Midi Message, but only for completeness.
        // Should never happen.
        _ => { format!("Failed to match message.") }
    }
}

// Helper functions
// In C, I'd memcpy two uint8 bytes into a pointer to a uint16, but give there's no
// memcpy here (well, without `unsafe`) I'm using silly bit tricks to do number conversions.
// Got these from how Rust io::net parses IP addresses.
fn u16_from_u8_at(buf : &[u8], offset : u32) -> u16 {
   (buf[offset] as u16 << 8) | (buf[offset + 1] as u16)
}

fn u32_from_u8_at(buf : &[u8], offset : u32) -> u32 {
   (buf[offset] as u32 << 24)
   | (buf[offset + 1] as u32 << 16)
   | (buf[offset + 2] as u32 << 8)
   | (buf[offset + 3] as u32)
}

fn get_track_size(buf : &[u8], offset : u32) -> u32 {
    let size = u32_from_u8_at(buf, offset + 4);
    size
}

fn file_format_from_u16(value : u16) -> Option<FileFormat> {
    match value {
        1 => Some(SingleTrack),
        2 => Some(MultipleSynchronous),
        3 => Some(MultipleAsynchronous),
        _ => None
    }
}

fn msb_is_one(number : u8) -> bool {
    number > 127
}
fn lower_seven_bits(number : u8) -> u8 {
    number & 0b01111111
}

fn is_invalid_status_byte(byte : u8) -> bool {
    match byte {
        0 .. 0x7F | 0xF4 | 0xF5 | 0xF7 | 0xF9 => true,
        _ => false
    }
}


fn get_status_byte(message : MidiMessage) -> u8 {
    match message {
        NoteOff         { channel : c, _ } => { 0x80 | c }
        NoteOn          { channel : c, _ } => { 0x90 | c }
        Aftertouch      { channel : c, _ } => { 0xA0 | c }
        ControlChange   { channel : c, _ } => { 0xB0 | c }
        ProgramChange   { channel : c, _ } => { 0xC0 | c }
        ChannelPressure { channel : c, _ } => { 0xD0 | c }
        PitchWheel      { channel : c, _ } => { 0xE0 | c }

        SystemExclusive     {_} => { 0xF0 }
        MidiTimeCode        {_} => { 0xF1 }
        SongPositionPointer {_} => { 0xF2 }
        SongSelect          {_} => { 0xF3 }
        TuneRequest             => { 0xF6 }
        MidiClock               => { 0xF8 }
        MidiStart               => { 0xFA }
        MidiContinue            => { 0xFB }
        MidiStop                => { 0xFC }
        ActiveSense             => { 0xFE }
        Reset                   => { 0xFF }
        // InvalidStatus gets an invalid Midi Message, but only for completeness.
        // Should never happen.
        _ => { 0xFD }
    }
}


// Writing
// Undefined for now, since we just want to read.



// Tests!
// Note that for tests that test external-facing definitions (mostly, parse_file), we should be
// writing that in a separate file called `test.rs` that imports these definitions. The following
// are just tests for the internal functions -- parse_ticks, parse_tracks, etc.

#[test]
fn test_parse_header_standard() {
   let test1 = [0x4D, 0x54, 0x68, 0x64, 0x00, 0x00, 0x00, 0x06,
                0x00, 0x01,
                0x00, 0x05,
                0x00, 0xa0];
   let rslt = parse_header(test1);
   match rslt {
       None => { assert!(false) }
       Some(x) => {
            assert!(x.num_tracks == 5);
            assert!(x.ticks_per_quarter == 160);
            match x.file_format {
                SingleTrack => assert!(true),
                _ => assert!(false)
            }
       }
   }

   let test2  = [0x4D, 0x54, 0x68, 0x64, 0x00, 0x00, 0x00, 0x06,
                 0x00, 0x02,
                 0x0a, 0x00,
                 0x01, 0x00];
   let rslt2 = parse_header(test2);
   match rslt2 {
       None => { assert!(false) }
       Some(x) => {
            assert!(x.num_tracks == 2560);
            assert!(x.ticks_per_quarter == 256);
            match x.file_format {
                MultipleSynchronous => assert!(true),
                _ => assert!(false)
            }
       }
   }
}

#[test]
fn test_parse_header_fail() {
    let test3  = [0x4D, 0x34, 0x68, 0x64, 0x00, 0x00, 0x00, 0x06,
                 0x00, 0x02,
                 0x0a, 0x00,
                 0x01, 0x00];
   let rslt3 = parse_header(test3);
   match rslt3 {
       None => { assert!(true) }
       Some(_) => { assert!(false) }
   }
}

#[test]
fn test_parse_ticks_easy() {
    let test_buf = [0x50, 0x90, 0x26, 0x3C];
    match parse_ticks(test_buf, 0) {
        (ticks, new_offset) => {
            assert!(ticks == 80);
            assert!(new_offset == 1);
        }
    }
}

#[test]
fn test_parse_ticks_hard() {
    let test_buf = [0x83, 0x60, 0x26, 0x00];
    match parse_ticks(test_buf, 0) {
        (ticks, new_offset) => {
            assert!(ticks == 480);
            assert!(new_offset == 2);
        }
    }
}

#[test]
fn test_parse_event_one() {
    let test_buf = [0x88, 0x05, 0x03];
    match parse_message(test_buf, 0, 0x80) {
        Some((NoteOff{channel : c, key : k, velocity : v}, 3)) => {
            assert!(c == 8);
            assert!(k == 5);
            assert!(v == 3);
        }
        _ => { assert!(false); }
    }
}

#[test]
fn test_parse_event_two() {
    let test_buf = [0xA3, 0x04, 0x09];
    match parse_message(test_buf, 0, 0x80) {
        Some((Aftertouch{channel : c, key : k, velocity : v}, 3)) => {
            assert!(c == 3);
            assert!(k == 4);
            assert!(v == 9);
        }
        _ => { assert!(false); }
    }
}

#[test]
fn test_parse_track_all_complete() {
    // This track contains 4 events: NoteOn, PitchWheel, Aftertouch, NoteOff. All events are spelled
    // out completely -- MIDI allows events to omit the event, to mean, 'do the last one.'
    let test_buf = [('M' as u8), ('T' as u8), ('r' as u8), ('k' as u8), 

        0x00, 0x00, 0x00, 0x11, // Track length: 17

        0x50,                   // Delta time: 80
        0x92, 0x05, 0x04,       // NoteOn, channel 2, key 5, velocity 4       

        0x50,                   // Delta time: 80
        0xE2, 0x06, 0x03,       // PitchWheel, channel 2, lsb 6, msb 3

        0x83, 0x60,             // Delta time: 480
        0xA2, 0x05, 0x04,       // Aftertouch, channel 2, key 5, velocity 4

        0x50,                   // Delta time: 80
        0x82, 0x05, 0x04        // NoteOff, channel 2, key 5, velocity 4
        ];

    match parse_track(test_buf, 0) {
        Some(track) => {
            assert!(track.track_length == 17);
            
            assert!(track.events[0].delta_time == 80);
            match track.events[0].message {
                NoteOn{ channel : c, key : k, velocity : v } => {
                    assert!(c == 2);
                    assert!(k == 5);
                    assert!(v == 4);
                }
                _ => { assert!(false) }
            }

            assert!(track.events[1].delta_time == 80);
            match track.events[1].message {
                PitchWheel{ channel : c, lsb : l, msb : m } => {
                    assert!(c == 2);
                    assert!(l == 6);
                    assert!(m == 3);
                }
                _ => { assert!(false) }
            }

            assert!(track.events[2].delta_time == 480);
            match track.events[2].message {
                Aftertouch{ channel : c, key : k, velocity : v } => {
                    assert!(c == 2);
                    assert!(k == 5);
                    assert!(v == 4);
                }
                _ => { assert!(false) }
            }

            assert!(track.events[3].delta_time == 80);
            match track.events[3].message {
                NoteOff{ channel : c, key : k, velocity : v } => {
                    assert!(c == 2);
                    assert!(k == 5);
                    assert!(v == 4);
                }
                _ => { assert!(false) }
            }
        }
        _ => { assert!(false); }
    }
}

#[test]
fn test_parse_track_some_ommitted() {
    // This track contains 4 events: NoteOn, NoteOn, Aftertouch, Aftertouch. Sequential events are
    // ommitted.
    let test_buf = [('M' as u8), ('T' as u8), ('r' as u8), ('k' as u8),

        0x00, 0x00, 0x00, 0x0F, // Track length: 15

        0x50,                   // Delta time: 80
        0x92, 0x05, 0x04,       // NoteOn, channel 2, key 5, velocity 4

        0x83, 0x60,             // Delta time: 480
        0x26, 0x00,             // Omit status (NoteOn), channel 2, key 38, velocity 0

        0x50,                   // Delta time: 80
        0xA2, 0x05, 0x04,       // Aftertouch, channel 2, key 5, velocity 4

        0x50,                   // Delta time: 80
        0x13, 0x05              // Omit status (Aftertouch), channel 2, key 19, velocity 5
        ];

    match parse_track(test_buf, 0) {
        Some(track) => {
            assert!(track.track_length == 15);

            assert!(track.events[0].delta_time == 80);
            match track.events[0].message {
                NoteOn{ channel : c, key : k, velocity : v } => {
                    assert!(c == 2);
                    assert!(k == 5);
                    assert!(v == 4);
                }
                _ => { assert!(false) }
            }

            assert!(track.events[1].delta_time == 480);
            match track.events[1].message {
                NoteOn{ channel : c, key : k, velocity : v } => {
                    assert!(c == 2);
                    assert!(k == 38);
                    assert!(v == 0);
                }
                _ => { assert!(false) }
            }

            assert!(track.events[2].delta_time == 80);
            match track.events[2].message {
                Aftertouch{ channel : c, key : k, velocity : v } => {
                    assert!(c == 2);
                    assert!(k == 5);
                    assert!(v == 4);
                }
                _ => { assert!(false) }
            }

            assert!(track.events[3].delta_time == 80);
            match track.events[3].message {
                Aftertouch{ channel : c, key : k, velocity : v } => {
                    assert!(c == 2);
                    assert!(k == 19);
                    assert!(v == 5);
                }
                _ => { assert!(false) }
            }
        }
        _ => { assert!(false); }
    }
}
