// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

use super::*;
use af27_core::session::Session;

fn read_s(xml: &str) -> (Session, Report) {
    read_report(xml, Path::new("."), "Test", Surrounds::Behind).unwrap()
}

fn wrap(session_attrs: &str, tracks: &str, files: &str) -> String {
    format!(
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\" ?>\n<!DOCTYPE sesx>\n<sesx version=\"1.9\"><session {session_attrs}><tracks>{tracks}</tracks><sessionState ctiPosition=\"96000\" smpteStart=\"0\"/></session><files>{files}</files></sesx>"
    )
}

const FADER: &str = r#"<component componentID="Audition.Fader" id="trackFader" name="volume" powered="true"><parameter index="0" name="volume" parameterValue="1.5848932"/><parameter index="1" name="static gain" parameterValue="1"/></component>"#;

fn sample() -> String {
    let tracks = format!(
        r#"<audioTrack id="10001" index="1" preRendered="true">
          <trackParameters trackHeight="117" trackHue="120"><name>HOST &amp; GUEST</name></trackParameters>
          <trackAudioParameters audioChannelType="mono" recordArmed="true" solo="true">
            <trackOutput outputID="10003" type="trackID"/>
            {FADER}
            <component componentID="Audition.Mute" id="trackMute" name="Mute" powered="true"><parameter index="1" name="mute" parameterValue="1"/></component>
            <component componentID="Audition.StereoPanner" id="trackPan" name="StereoPanner" powered="true"><parameter index="0" name="Pan" parameterValue="-50"/></component>
            <sendList><sendSlot preFader="true" sendOutputID="10003" slotIndex="0">
              <component componentID="Audition.Send" id="sendComponent" powered="true"><parameter index="0" name="Power On/Off" parameterValue="1"/><parameter index="1" name="Level" parameterValue="0.5"/></component>
              <component componentID="Audition.StereoPanner" id="sendPan" powered="true"><parameter index="0" name="Pan" parameterValue="25"/></component>
            </sendSlot><sendSlot preFader="false" sendOutputID="0" slotIndex="1"><component id="sendComponent" powered="false"><parameter index="0" name="Power On/Off" parameterValue="0"/><parameter index="1" name="Level" parameterValue="0"/></component></sendSlot></sendList>
          </trackAudioParameters>
          <effectsRack preFader="true">
            <component id="rackInput" powered="true"><parameter index="0" parameterValue="1"/></component>
            <component id="rackOutput" powered="true"><parameter index="0" parameterValue="0.4217"/></component>
            <component id="rackMix" powered="true"><parameter index="0" parameterValue="0.25"/></component>
            <slot powered="true" slotIndex="3"><component componentID="3:56535441 54383141 75746f2d 54756e65" name="Tuner"><presetChunkData><![CDATA[{state}]]></presetChunkData></component></slot>
            <slot powered="false" slotIndex="1"><component componentID="e0b23f05-f1a7-4ef7-9b50-7ec3e3002058" name="Hard Limiter"><parameter index="0" name="Maximum Amplitude" parameterValue="0.94"/><parameter index="1" name="Input Boost" parameterValue="0.6666667"/></component></slot>
            <slot powered="true" slotIndex="2"><component componentID="31d33f85-3288-489e-8390-d03c60d9ce6c" name="Chorus"/></slot>
          </effectsRack>
          <audioClip fileID="0" id="1" name="second" zOrder="1" sourceInPoint="0" sourceOutPoint="44100" startPoint="88200" endPoint="132300" hue="-1"/>
          <audioClip fileID="0" id="0" name="host take" zOrder="0" lockedInTime="true" hue="0"
                     sourceInPoint="10290" sourceOutPoint="20580" startPoint="44100" endPoint="54390">
            <component componentID="Audition.Fader" id="clipGain" name="volume" powered="true"><parameter index="0" name="volume" parameterValue="0.5"/><parameter index="1" name="static gain" parameterValue="0.5"/></component>
            <clipStretch stretchRatio="1.029" stretchType="solo" stretchMode="realtime" pitchAdjustment="2" adaptiveWindowSize="37" transientSensitivity="40" preserveFormants="true" stretchQuality="high"/>
            <channelMap><channel index="0" sourceIndex="1"/></channelMap>
            <fadeIn startPoint="0" endPoint="4410" type="log" shape="19"/>
            <fadeOut startPoint="5880" endPoint="10290" type="cosine" shape="0"/>
          </audioClip>
        </audioTrack>
        <busTrack id="10003" index="2">
          <trackParameters><name>Bus A</name></trackParameters>
          <trackAudioParameters><trackOutput outputID="10000" type="trackID"/></trackAudioParameters>
        </busTrack>
        <videoTrack id="10004" index="3"/>
        <masterTrack id="10000" index="4">
          <trackAudioParameters>
            <component componentID="Audition.Fader" id="trackFader" name="volume" powered="true"><parameter index="0" name="volume" parameterValue="0.5"/></component>
          </trackAudioParameters>
        </masterTrack>"#,
        state = crate::b64::encode(&chunk_bytes(b"COMP", b"CT"))
    );
    wrap(
        r#"audioChannelType="stereo" bitDepth="24" duration="400000" sampleRate="44100""#,
        &tracks,
        r#"<file id="0" absolutePath="Z:\nowhere\host.wav" relativePath="Raw/host.wav" importerPrivateSettings="Channels:0:2;SampleRate:0:48000;"/>"#,
    )
}

fn chunk_bytes(comp: &[u8], ctl: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend((comp.len() as u64).to_le_bytes());
    v.extend((ctl.len() as u64).to_le_bytes());
    v.extend(comp);
    v.extend(ctl);
    v
}

#[test]
fn tracks_buses_clips_and_media_come_across() {
    let (s, r) = read_s(&sample());
    assert_eq!(s.rate, 44_100);
    assert_eq!(s.bit_depth, 24);
    assert!((s.playhead - 96000.0 / 44100.0).abs() < 1e-9);
    assert_eq!(s.tracks.len(), 2);
    let t = &s.tracks[0];
    assert_eq!(t.name, "HOST & GUEST");
    assert_eq!(t.height, 117.0);
    assert!(t.mute && t.solo && t.armed && t.mono);
    assert!((t.volume_db - 4.0).abs() < 0.01, "{}", t.volume_db);
    assert!((t.pan + 0.5).abs() < 1e-6);
    assert_eq!(t.color, [108, 217, 108]);
    let bus = &s.tracks[1];
    assert_eq!(bus.kind, TrackKind::Bus);
    assert_eq!(t.output, Some(bus.id));
    assert_eq!(bus.output, None, "the master");
    // One real send (the empty slot pointing nowhere is not one).
    assert_eq!(t.sends.len(), 1);
    let sd = &t.sends[0];
    assert_eq!(sd.bus, bus.id);
    assert!(sd.pre_fader && sd.enabled);
    assert!((sd.level_db + 6.02).abs() < 0.01 && (sd.pan - 0.25).abs() < 1e-6);
    assert!((s.master_db + 6.02).abs() < 0.01);
    // Clips by zOrder; positions in session samples; source points in
    // stretched time.
    let c = &t.clips[0];
    assert_eq!(c.name, "host take");
    assert!((c.start - 1.0).abs() < 1e-9);
    assert!((c.length - 10290.0 / 44100.0).abs() < 1e-9);
    assert!((c.offset - 10000.0 / 44100.0).abs() < 1e-6, "{}", c.offset);
    assert!((c.gain_db + 12.04).abs() < 0.01, "{}", c.gain_db);
    assert!(c.props.locked);
    assert_eq!(c.props.color, Some([217, 108, 108]));
    let st = c.props.stretch;
    assert_eq!(st.kind, StretchKind::Monophonic);
    assert_eq!(st.mode, StretchMode::Live);
    assert!((st.ratio - 1.029).abs() < 1e-9 && (st.pitch - 2.0).abs() < 1e-6);
    assert_eq!(st.window_ms, 37.0);
    assert!((c.fade_in - 0.1).abs() < 1e-9 && c.props.fade_in_skew == 19.0);
    assert!((c.fade_out - 0.1).abs() < 1e-9 && c.props.fade_out_skew == 0.0);
    assert_eq!(t.clips[1].name, "second");
    // The file is missing: a placeholder as long as its clips need.
    let m = &s.media[0];
    assert_eq!(m.channels, 2);
    assert_eq!(m.rate, 48_000);
    assert!((m.duration - 1.0).abs() < 1e-6, "{}", m.duration);
    assert_eq!(r.missing_files, 1);
    assert_eq!(
        (r.tracks, r.buses, r.clips, r.sends, r.stretched),
        (1, 1, 2, 1, 1)
    );
    for what in ["video track(s)", "Pre-render switch(es)"] {
        assert_eq!(r.dropped.get(what), Some(&1), "{what}: {:?}", r.dropped);
    }
    // Source Channels: the right side of the stereo file on both sides.
    let map = c.props.channel_map.as_ref().expect("a channel map");
    assert_eq!(map.len(), 2);
    assert_eq!((map[0].left, map[0].right), (0.0, 0.0));
    assert_eq!((map[1].left, map[1].right), (1.0, 1.0));
    assert!(r.dropped.keys().all(|k| !k.contains("Source Channels")));
    assert!(r.approximated.keys().any(|k| k.starts_with("S-curve")));
    let n = r.notices().join("\n");
    assert!(
        n.contains("Imported 1 track(s), 1 bus(es) and 2 clip(s)."),
        "{n}"
    );
    assert!(n.contains("Chorus") && n.contains("not found"), "{n}");
}

#[test]
fn racks_plug_ins_and_conversions() {
    let (s, r) = read_s(&sample());
    let t = &s.tracks[0];
    assert_eq!(t.rack.input_db, 0.0);
    assert_eq!(t.rack.output_db, -7.5);
    assert_eq!(t.rack.mix, 0.75);
    // Sorted by slot number, keeping it; the Chorus is left out.
    assert_eq!(t.chain.len(), 2);
    let lim = &t.chain[0];
    assert_eq!(lim.effect, "limiter");
    assert_eq!(lim.position, Some(1));
    assert!(lim.bypass, "its slot was off");
    assert_eq!(lim.values[0], -6.0);
    assert_eq!(lim.values[1], 0.0);
    let plug = &t.chain[1];
    assert_eq!(plug.position, Some(3));
    let want = if cfg!(windows) {
        "vst3:41545356385441317574"
    } else {
        "vst3:56535441543831417574"
    };
    assert!(plug.effect.starts_with(want), "{}", plug.effect);
    let d = plug.plugin_data();
    assert_eq!(d.name, "Tuner");
    let state = crate::b64::decode(d.state.as_deref().unwrap()).unwrap();
    assert_eq!(state, [&4u32.to_le_bytes()[..], b"COMP", b"CT"].concat());
    assert_eq!(r.plugins, vec!["Tuner".to_string()]);
    assert_eq!(r.skipped_names, vec!["Chorus".to_string()]);
}

#[test]
fn class_ids_and_states_convert_both_ways() {
    let id = "3:56535441 54383141 75746f2d 54756e65";
    for com in [true, false] {
        let e = vst3_effect(id, com).unwrap();
        assert_eq!(crate::sesx_out::vst3_component(&e, com).unwrap(), id);
    }
    assert_eq!(
        vst3_effect(id, false).unwrap(),
        "vst3:56535441543831417574 6F2D54756E65".replace(' ', "")
    );
    let chunk = chunk_bytes(b"abc", b"de");
    let st = chunk_to_state(&chunk).unwrap();
    assert_eq!(state_to_chunk(&st).unwrap(), chunk);
    // Lengths that do not add up drop only the state.
    let mut bad = chunk.clone();
    bad.push(0);
    assert!(chunk_to_state(&bad).is_none());
    assert!(vst3_effect("3:zz", true).is_none());
}

#[test]
fn limiter_presets() {
    let p = |a: f64| HashMap::from([("Maximum Amplitude".to_string(), a)]);
    assert_eq!(limiter(&p(0.94)).values[0], -6.0);
    assert_eq!(limiter(&p(0.97)).values[0], -3.0);
    assert_eq!(limiter(&p(1.0)).values[0], 0.0);
}

#[test]
fn parametric_eq_knobs_become_values_in_range() {
    let p: HashMap<String, f64> = [
        ("Original Sample Rate", 44_100.0 / 192_000.0),
        ("Gain", 2.0 / 3.0),
        ("EQ Band 2 Enable", 1.0),
        ("EQ Band 2 Center Frequency", 0.5),
        ("EQ Band 2 Gain", 0.6),
        ("EQ Band 2 Q", 0.0001),
        ("EQ Band 3 Gain", 1.0),
        ("High-Pass Slope", 0.4),
        ("High Shelf Frequency", 0.0),
        ("Bypass", 1.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let (v, bypass) = parametric_eq(&p);
    assert!(bypass);
    assert_eq!(v["gain"], 0.0);
    assert_eq!(v["b2_on"], 1.0);
    assert_eq!(v["b2_freq"], (20.0 + 0.5 * (22_050.0 - 20.0f64)).round());
    assert!((v["b2_gain"] - 9.6).abs() < 1e-9);
    assert!((v["b2_q"] - 1.0).abs() < 1e-9);
    assert_eq!(v["b3_gain"], 24.0, "+48 dB is kept within ±24");
    assert_eq!(v["hp_slope"], 1.0, "0.4 is 24 dB/oct");
    assert_eq!(v["hs_freq"], 1000.0, "kept within the shelf's range");
}

#[test]
fn phase_shifter_curves() {
    // 48 kHz, right channel, axis 0..1 of half the rate, 3 points.
    let mut f = [0f32; 17];
    f[0] = 48_000.0;
    f[1] = 1.0;
    f[6] = 20.0 / 24_000.0;
    f[7] = 1.0;
    f[10] = 3.0;
    f[11..17].copy_from_slice(&[0.0, 0.5, 0.5, 1.0, 1.0, 0.25]);
    let bytes: Vec<u8> = f.iter().flat_map(|x| x.to_le_bytes()).collect();
    let s = phase_curve(&bytes).unwrap();
    let info = af27_fx::info(&s.effect).unwrap();
    let get = |k: &str| s.values[info.params.iter().position(|p| p.key == k).unwrap()];
    assert_eq!(get("points"), 3.0);
    assert_eq!(get("channels"), 2.0);
    assert_eq!(get("spline"), 0.0);
    assert_eq!(get("p1_freq"), 20.0);
    assert_eq!(get("p1_phase"), 0.0);
    assert_eq!(get("p2_phase"), 180.0);
    assert!((get("p2_freq") - (20.0f64 * 1200.0f64.sqrt()) as f32).abs() < 0.2);
    assert_eq!(get("p3_phase"), -90.0);
    // Too many points are thinned to sixteen, keeping both ends.
    let n = 40usize;
    let mut f = vec![0f32; 11 + 2 * n];
    f[0] = 44_100.0;
    f[6] = 0.001;
    f[7] = 1.0;
    f[10] = n as f32;
    for i in 0..n {
        f[11 + 2 * i] = i as f32 / (n - 1) as f32;
        f[12 + 2 * i] = 0.5;
    }
    let bytes: Vec<u8> = f.iter().flat_map(|x| x.to_le_bytes()).collect();
    let s = phase_curve(&bytes).unwrap();
    assert_eq!(s.values[0], 16.0);
    assert!(phase_curve(&[0u8; 8]).is_none());
}

#[test]
fn vox_strip_knobs() {
    let xml = br#"junk<XVox><PARAM id="input_gain" value="2"/><PARAM id="hpf" value="1"/><PARAM id="dyn_comp" value="0.42"/><PARAM id="tone_low" value="-0.5"/><PARAM id="globalEnable" value="0"/><PARAM id="space_mix" value="1"/></XVox>"#;
    let s = vox_strip(xml).unwrap();
    let info = af27_fx::info(&s.effect).unwrap();
    let get = |k: &str| s.values[info.params.iter().position(|p| p.key == k).unwrap()];
    assert!((get("input") - 6.02).abs() < 0.01);
    assert_eq!(get("low_cut"), 1.0);
    assert_eq!(get("comp"), 42.0);
    assert_eq!(get("low"), -6.0);
    assert!(s.bypass);
    assert!(vox_strip(b"nothing here").is_none());
}

#[test]
fn levels_and_drawn_lines() {
    let tracks = r#"<audioTrack id="10001" index="1"><trackParameters><name>T</name></trackParameters>
      <component componentID="Audition.Fader" id="trackFader" name="volume" powered="true">
        <parameter index="0" name="volume" parameterValue="1"><parameterKeyframes>
          <parameterKeyframe sampleOffset="44100" type="linear" value="0.5"/>
          <parameterKeyframe sampleOffset="0" type="hold" value="1"/>
        </parameterKeyframes></parameter>
        <parameter index="1" name="static gain" parameterValue="0.5"/></component>
      <trackAudioParameters><trackOutput outputID="10000" type="trackID"/>
        <component componentID="Audition.StereoPanner" id="trackPan" powered="true"><parameter index="0" name="Pan" parameterValue="0">
          <parameterKeyframe sampleOffset="0" type="linear" value="0.75"/></parameter></component>
        <component componentID="x" name="Delay"><parameter name="Mix" parameterValue="1"><parameterKeyframe sampleOffset="0" value="1"/></parameter></component>
      </trackAudioParameters>
      <audioClip fileID="0" startPoint="0" endPoint="100" sourceInPoint="0" sourceOutPoint="100">
        <component componentID="Audition.Fader" id="clipGain" powered="true"><parameter index="0" name="volume" parameterValue="1">
          <parameterKeyframe sampleOffset="0" value="0.5"/><parameterKeyframe sampleOffset="50" value="0.5"/></parameter></component></audioClip>
      <audioClip fileID="0" startPoint="200" endPoint="300" sourceInPoint="0" sourceOutPoint="100">
        <component componentID="Audition.Fader" id="clipGain" powered="true"><parameter index="0" name="volume" parameterValue="1">
          <parameterKeyframe sampleOffset="0" value="0.5"/><parameterKeyframe sampleOffset="50" value="1"/></parameter></component></audioClip>
      <audioClip startPoint="0" endPoint="10"/>
    </audioTrack><masterTrack id="10000"/>"#;
    let (s, r) = read_s(&wrap(
        r#"sampleRate="44100""#,
        tracks,
        r#"<file id="0" absolutePath="x.wav"/>"#,
    ));
    let t = &s.tracks[0];
    assert!(
        (t.volume_db + 6.02).abs() < 0.01,
        "static gain is the fixed level"
    );
    let vol = &t.lanes[0];
    assert_eq!(vol.target, LaneTarget::Volume);
    assert_eq!(vol.points.len(), 2);
    assert_eq!((vol.points[0].time, vol.points[0].value), (0.0, 0.0));
    assert_eq!(vol.points[0].curve, Curve::Hold);
    assert!((vol.points[1].time - 1.0).abs() < 1e-9 && (vol.points[1].value + 6.02).abs() < 0.01);
    let pan = &t.lanes[1];
    assert_eq!(pan.target, LaneTarget::Pan);
    assert_eq!(pan.points[0].value, 0.5);
    // A flat clip line is a level; a moving one is the clip's own lane.
    assert!((t.clips[0].gain_db + 6.02).abs() < 0.01);
    assert!(t.clips[0].props.lanes.is_empty());
    assert_eq!(t.clips.len(), 2, "a clip without a file is skipped");
    let cl = &t.clips[1].props.lanes;
    assert_eq!(cl.len(), 1);
    assert_eq!(cl[0].target, LaneTarget::Volume);
    // Cut to the clip (100 samples): the points, and the end held.
    let pts: Vec<(f64, f64)> = cl[0]
        .points
        .iter()
        .map(|p| {
            (
                (p.time * 44100.0).round(),
                (p.value * 100.0).round() / 100.0,
            )
        })
        .collect();
    assert_eq!(pts, vec![(0.0, -6.02), (50.0, 0.0), (100.0, 0.0)]);
    assert!(!r.dropped.contains_key("clip volume line(s)"));
    assert_eq!(r.dropped.get("other automation line(s)"), Some(&1));
    assert_eq!(r.lines, 3);
}

#[test]
fn markers_come_from_the_xmp_block() {
    let xmp = r#"<xmpMetadata><![CDATA[<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:xmpDM="http://ns.adobe.com/xmp/1.0/DynamicMedia/">
<rdf:Description><xmpDM:Tracks><rdf:Bag>
<rdf:li rdf:parseType="Resource"><xmpDM:trackName>CuePoint Markers</xmpDM:trackName><xmpDM:frameRate>f44100</xmpDM:frameRate>
<xmpDM:markers><rdf:Seq>
<rdf:li rdf:parseType="Resource"><xmpDM:startTime>88200</xmpDM:startTime><xmpDM:name>Verse</xmpDM:name></rdf:li>
<rdf:li rdf:parseType="Resource"><xmpDM:startTime>44100</xmpDM:startTime><xmpDM:duration>22050</xmpDM:duration><xmpDM:name>Intro</xmpDM:name></rdf:li>
</rdf:Seq></xmpDM:markers></rdf:li></rdf:Bag></xmpDM:Tracks></rdf:Description></rdf:RDF></x:xmpmeta>
<?xpacket end="w"?>]]></xmpMetadata>"#;
    let xml =
        wrap(r#"sampleRate="48000""#, "", "").replace("</session>", &format!("{xmp}</session>"));
    let (s, r) = read_s(&xml);
    assert_eq!(r.markers, 2);
    assert_eq!(s.markers[0].name, "Intro");
    assert_eq!((s.markers[0].time, s.markers[0].length), (1.0, 0.5));
    assert_eq!(s.markers[1].time, 2.0);
}

#[test]
fn five_one_panners_become_room_spots() {
    let panner = |angle: f64, spread: f64| {
        format!(
            r#"<audioTrack id="1" index="1"><trackAudioParameters audioChannelType="stereo"><trackOutput outputID="9" type="trackID"/>
            <component componentID="Audition.StereoTo51Panner" name="StereoTo51Panner"><parameter name="Pan: Angle" parameterValue="{angle}"/>
            <parameter name="Pan: Radius" parameterValue="1"/><parameter name="Pan: Stereo Spread" parameterValue="{spread}"/><parameter name="Pan: LFE" parameterValue="0.25"/></component>
            </trackAudioParameters></audioTrack>
            <busTrack id="2" index="2"><trackAudioParameters audioChannelType="fivePointOne"/></busTrack>"#
        )
    };
    let xml = |a: f64, sp: f64| {
        wrap(
            r#"audioChannelType="fivePointOne" sampleRate="48000""#,
            &panner(a, sp),
            "",
        )
    };
    assert!(is_surround(&xml(0.0, 0.0)));
    assert!(!is_surround(&sample()));
    let front = std::f64::consts::FRAC_PI_2;
    let (s, _) = read_s(&xml(front, 0.0));
    assert_eq!(s.space, Some(SpaceLayout::Surround51));
    let p = s.tracks[0].pos;
    assert!(p.x.abs() < 1e-9 && (p.y - 1.0).abs() < 1e-9, "{p:?}");
    assert_eq!(p.lfe, 0.25);
    assert!(s.tracks[1].spatial.room, "a 5.1 bus is a 3D Bus");
    // 30 degrees round from the front lands in the room's corner.
    let (s, _) = read_s(&xml(front + 30f64.to_radians(), 0.0));
    let p = s.tracks[0].pos;
    assert!(
        (p.x.abs() - 1.0).abs() < 1e-9 && (p.y - 1.0).abs() < 1e-9,
        "{p:?}"
    );
    // 110 degrees: beside (Sides) or a back corner (Behind).
    let at110 = |sur| {
        read_report(
            &xml(front + 110f64.to_radians(), 0.0),
            Path::new("."),
            "t",
            sur,
        )
        .unwrap()
        .0
        .tracks[0]
            .pos
    };
    let b = at110(Surrounds::Behind);
    assert!(
        (b.y + 1.0).abs() < 1e-9 && (b.x.abs() - 1.0).abs() < 1e-9,
        "{b:?}"
    );
    let sd = at110(Surrounds::Sides);
    assert!(
        sd.y.abs() < 1e-9 && (sd.x.abs() - 1.0).abs() < 1e-9,
        "{sd:?}"
    );
    // A stereo spread opens the pair across the front.
    let (s, _) = read_s(&xml(front, 60f64.to_radians()));
    let p = s.tracks[0].pos;
    assert!(
        (p.width - 1.0).abs() < 1e-9 && p.rotation.abs() < 1e-9,
        "{p:?}"
    );
}

#[test]
fn not_a_session() {
    assert!(read("<notsesx/>", Path::new("."), "x", Surrounds::Behind).is_err());
    assert!(read("not xml", Path::new("."), "x", Surrounds::Behind).is_err());
    let utf16: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain(
            "<sesx><session sampleRate=\"8000\"/></sesx>"
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes()),
        )
        .collect();
    let text = decode(&utf16);
    assert_eq!(read_s(&text).0.rate, 8000);
}

#[test]
fn media_is_found_relative_then_absolute_then_beside() {
    let dir = std::env::temp_dir().join(format!("af27-sesx-{}", crate::audfrm::new_uuid()));
    std::fs::create_dir_all(dir.join("Raw")).unwrap();
    std::fs::write(dir.join("Raw").join("a.wav"), b"x").unwrap();
    std::fs::write(dir.join("b.wav"), b"x").unwrap();
    let files = r#"<file id="0" absolutePath="Q:\gone\a.wav" relativePath="Raw\a.wav"/><file id="1" absolutePath="Q:\gone\b.wav" relativePath="x/b.wav"/>"#;
    let tracks = r#"<audioTrack id="1" index="1"><audioClip fileID="0" startPoint="0" endPoint="10"/><audioClip fileID="1" startPoint="20" endPoint="30"/></audioTrack>"#;
    let (s, r) = read_report(
        &wrap(r#"sampleRate="48000""#, tracks, files),
        &dir,
        "t",
        Surrounds::Behind,
    )
    .unwrap();
    assert_eq!(r.missing_files, 0);
    assert_eq!(
        s.media[0].path.as_deref(),
        Some(dir.join("Raw").join("a.wav").as_path())
    );
    assert_eq!(
        s.media[1].path.as_deref(),
        Some(dir.join("b.wav").as_path())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------- export

#[test]
fn an_export_reads_back_the_same() {
    let dir = std::env::temp_dir().join(format!("af27-sesxout-{}", crate::audfrm::new_uuid()));
    std::fs::create_dir_all(dir.join("Audio")).unwrap();
    let wav = dir.join("Audio").join("host & guest.wav");
    std::fs::write(&wav, b"x").unwrap();
    let (mut s, _) = read_s(&sample());
    s.media[0].path = Some(wav.clone());
    s.tracks[0].pan = 0.01;
    s.tracks[0].lanes.clear();
    let mut lane = Lane::new(LaneTarget::Volume);
    lane.points = vec![
        LanePoint {
            time: 0.0,
            value: -3.0,
            curve: Curve::Linear,
        },
        LanePoint {
            time: 1.0,
            value: -9.0,
            curve: Curve::Linear,
        },
    ];
    s.tracks[0].lanes.push(lane);
    s.tracks[0].clips[1].props.reversed = true;
    s.markers.push(Marker {
        name: "m".into(),
        time: 1.0,
        length: 0.0,
        ext_id: None,
    });
    let out = dir.join("Copy.sesx");
    let rep = crate::sesx_out::export(&s, &out).unwrap();
    assert_eq!(rep.effects_left_out, 1, "the Limiter");
    assert_eq!(rep.clips_simplified, 1);
    assert_eq!(rep.markers_left_out, 1);
    assert!(rep.notice().unwrap().contains("marker"));
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(
        text.contains(r#"relativePath="Audio/host &amp; guest.wav""#),
        "{text}"
    );
    let (b, _) = read_report(&text, &dir, "Copy", Surrounds::Behind).unwrap();
    let (t, u) = (&s.tracks[0], &b.tracks[0]);
    assert_eq!(u.name, t.name);
    assert!(u.mono && u.mute && u.solo && u.armed);
    assert!((u.volume_db - t.volume_db).abs() < 0.01);
    assert!(
        (u.pan - 0.01).abs() < 1e-6,
        "pan as -100..100 keeps small values"
    );
    assert_eq!(u.lanes[0].points.len(), 2);
    assert!((u.lanes[0].points[1].value + 9.0).abs() < 0.01);
    assert_eq!(u.output, Some(b.tracks[1].id));
    assert_eq!(u.sends.len(), 1);
    assert!((u.sends[0].level_db - t.sends[0].level_db).abs() < 0.01);
    assert_eq!(u.rack, t.rack);
    assert_eq!(u.chain.len(), 1, "the plug-in");
    assert_eq!(u.chain[0].effect, t.chain[1].effect);
    assert_eq!(u.chain[0].position, Some(3));
    assert_eq!(
        u.chain[0].plugin_data().state,
        t.chain[1].plugin_data().state
    );
    for (a, c) in t.clips.iter().zip(&u.clips) {
        assert!((a.start - c.start).abs() < 1e-4);
        assert!((a.length - c.length).abs() < 1e-4);
        assert!(
            (a.offset - c.offset).abs() < 1e-4,
            "{} {}",
            a.offset,
            c.offset
        );
        assert!((a.gain_db - c.gain_db).abs() < 0.01);
        assert_eq!(a.props.locked, c.props.locked);
        assert!((a.fade_in - c.fade_in).abs() < 1e-4);
        assert_eq!(a.props.fade_in_skew, c.props.fade_in_skew);
        assert_eq!(a.props.stretch.kind, c.props.stretch.kind);
        assert!((a.props.stretch.ratio - c.props.stretch.ratio).abs() < 1e-9);
        assert_eq!(a.props.stretch.pitch, c.props.stretch.pitch);
    }
    assert_eq!(b.rate, s.rate);
    assert_eq!(b.bit_depth, 24);
    assert_eq!(b.media.len(), 1);
    assert_eq!(b.media[0].path.as_deref(), Some(wav.as_path()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Imports every `.sesx` under `AF27_SESX_DIR` (read only):
/// `cargo test -p af27-interop real_sesx -- --ignored --nocapture`.
#[test]
#[ignore]
fn real_sesx_sessions_import() {
    let Some(dir) = std::env::var_os("AF27_SESX_DIR") else {
        return;
    };
    let mut stack = vec![PathBuf::from(dir)];
    let (mut n, mut clips, mut failed) = (0, 0, Vec::new());
    let mut dropped: BTreeMap<String, usize> = BTreeMap::new();
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|f| f != "Backup") {
                    stack.push(p);
                }
                continue;
            }
            if !p
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("sesx"))
            {
                continue;
            }
            let bytes = std::fs::read(&p).unwrap();
            let text = decode(&bytes);
            match read_report(&text, p.parent().unwrap(), "t", Surrounds::Behind) {
                Ok((s, r)) => {
                    n += 1;
                    clips += r.clips;
                    for (k, v) in r.dropped {
                        *dropped.entry(k).or_default() += v;
                    }
                    for t in &s.tracks {
                        for c in &t.clips {
                            assert!(c.length > 0.0 && c.offset >= 0.0, "{}", p.display());
                        }
                    }
                    // An export reads back with the same clips.
                    let (out, _) = crate::sesx_out::to_sesx(&s, p.parent());
                    let (b, _) = read_report(&out, p.parent().unwrap(), "t", Surrounds::Behind)
                        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                    let count = |s: &Session| s.tracks.iter().map(|t| t.clips.len()).sum::<usize>();
                    assert_eq!(count(&b), count(&s), "{}", p.display());
                }
                Err(e) => failed.push(format!("{}: {e}", p.display())),
            }
        }
    }
    eprintln!("{n} sessions, {clips} clips imported; left out: {dropped:?}");
    assert!(failed.is_empty(), "{failed:#?}");
    assert!(n > 0);
}

#[test]
fn a_converted_eq_saves_for_audiform_with_its_keys() {
    let tracks = format!(
        r#"<audioTrack id="1" index="1"><effectsRack><slot slotIndex="0"><component componentID="{PARAMETRIC_EQ}" name="Parametric Equalizer"><parameter name="Gain" parameterValue="0.6666667"/><parameter name="EQ Band 1 Enable" parameterValue="1"/></component></slot></effectsRack></audioTrack>"#
    );
    let (mut s, r) = read_s(&wrap(r#"sampleRate="48000""#, &tracks, ""));
    assert_eq!(r.converted.len(), 1);
    let doc = crate::audfrm::export(&mut s, None);
    let slot = &doc["tracks"][0]["effects"]["slots"][0];
    assert_eq!(slot["effect"], "altaudio.parametric_eq");
    assert_eq!(slot["params"]["b1_on"], 1.0);
    assert_eq!(slot["position"], 0);
    assert!(slot.get("data").is_none());
}

#[test]
fn clip_lines_racks_and_source_channels_come_across_and_export_back() {
    // A clip 1 s into its file (sourceInPoint 44100), 1 s long: its lines
    // count from the file start, so they move to clip time and are cut.
    let tracks = r#"<audioTrack id="10001" index="1"><trackParameters><name>T</name></trackParameters>
      <trackAudioParameters audioChannelType="stereo"><trackOutput outputID="10000" type="trackID"/></trackAudioParameters>
      <audioClip fileID="0" startPoint="0" endPoint="44100" sourceInPoint="44100" sourceOutPoint="88200">
        <component componentID="Audition.Fader" id="clipGain" powered="true"><parameter index="0" name="volume" parameterValue="1">
          <parameterKeyframe sampleOffset="0" value="1"/><parameterKeyframe sampleOffset="66150" value="0.5"/></parameter>
          <parameter index="1" name="static gain" parameterValue="1"/></component>
        <component componentID="Audition.StereoPanner" id="clipPan" powered="true"><parameter index="0" name="Pan" parameterValue="0">
          <parameterKeyframe sampleOffset="44100" value="0"/><parameterKeyframe sampleOffset="88200" value="1"/></parameter></component>
        <effectsRack preFader="true">
          <component id="rackOutput" powered="true"><parameter index="0" parameterValue="0.5"/></component>
          <slot powered="true" slotIndex="2"><component componentID="3:56535441 54383141 75746f2d 54756e65" name="Tuner"/></slot>
        </effectsRack>
        <channelMap><channel index="0" sourceIndex="0"/></channelMap>
      </audioClip>
      <audioClip fileID="0" startPoint="88200" endPoint="132300" sourceInPoint="0" sourceOutPoint="44100">
        <component componentID="Audition.StereoPanner" id="clipPan" powered="true"><parameter index="0" name="Pan" parameterValue="-40"/></component>
      </audioClip>
    </audioTrack><masterTrack id="10000"/>"#;
    let (s, r) = read_s(&wrap(
        r#"sampleRate="44100""#,
        tracks,
        r#"<file id="0" absolutePath="Z:\nowhere\x.wav" importerPrivateSettings="Channels:0:2;"/>"#,
    ));
    assert!(r.dropped.is_empty(), "{:?}", r.dropped);
    let c = &s.tracks[0].clips[0];
    let vol = c.playing_lane(&LaneTarget::Volume).unwrap();
    let at = |l: &Lane, t: f64| l.value_at(t).unwrap();
    assert!(
        (at(vol, 0.0) + 6.02 * 2.0 / 3.0).abs() < 0.01,
        "{:?}",
        vol.points
    );
    assert!((at(vol, 0.5) + 6.02).abs() < 0.01, "{:?}", vol.points);
    assert!((at(vol, 1.0) + 6.02).abs() < 0.01);
    let pan = c.playing_lane(&LaneTarget::Pan).unwrap();
    assert!((at(pan, 0.0) + 1.0).abs() < 1e-6 && (at(pan, 1.0) - 1.0).abs() < 1e-6);
    assert_eq!(c.props.chain.len(), 1);
    assert_eq!(c.props.chain[0].position, Some(2));
    assert!((c.props.rack.output_db + 6.0).abs() < 0.1);
    // Only the left of the stereo file, on both sides.
    let map = c.props.channel_map.as_ref().unwrap();
    assert_eq!(
        map.iter().map(|m| (m.left, m.right)).collect::<Vec<_>>(),
        vec![(1.0, 1.0), (0.0, 0.0)]
    );
    // A fixed clip pan is a lane of one point.
    let p2 = s.tracks[0].clips[1].playing_lane(&LaneTarget::Pan).unwrap();
    assert_eq!(p2.points.len(), 1);
    assert!((p2.points[0].value + 0.4).abs() < 1e-6);

    // Exported and read back: the same lines, rack and map.
    let (out, rep) = crate::sesx_out::to_sesx(&s, None);
    assert_eq!(rep.lanes_left_out, 0);
    let (b, _) = read_s(&out);
    let cb = &b.tracks[0].clips[0];
    let vb = cb.playing_lane(&LaneTarget::Volume).unwrap();
    for t in [0.0, 0.25, 0.5, 0.9] {
        assert!((at(vb, t) - at(vol, t)).abs() < 0.02, "{t}");
    }
    let pb = cb.playing_lane(&LaneTarget::Pan).unwrap();
    assert!((at(pb, 0.5) - at(pan, 0.5)).abs() < 1e-3);
    assert_eq!(cb.props.chain.len(), 1);
    // The copy does not note the file's channel count: channels the map
    // leaves out are silent, as before.
    let mb = cb.props.channel_map.as_ref().unwrap();
    assert_eq!(mb[0], map[0]);
    assert!(map[mb.len()..]
        .iter()
        .all(|m| m.left == 0.0 && m.right == 0.0));
    let p2b = b.tracks[0].clips[1].playing_lane(&LaneTarget::Pan).unwrap();
    assert!((p2b.points[0].value + 0.4).abs() < 1e-6);
}
