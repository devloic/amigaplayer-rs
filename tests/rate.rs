//! The sample rate changes how finely the sound is sampled, not how fast or how high it plays.

use amigaplayer_rs::testdata::minimal_mod;
use amigaplayer_rs::Song;

/// Rising zero crossings of the left channel in the first `secs` seconds.
fn crossings(rate: u32, secs: f64) -> usize {
    let mut song = Song::open("a.mod", &minimal_mod(), rate).unwrap();
    let mut pcm = vec![0f32; 2 * (secs * rate as f64) as usize];
    song.render_f32(&mut pcm);
    let left: Vec<f32> = pcm.chunks(2).map(|f| f[0]).collect();
    left.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
}

#[test]
fn pitch_and_tempo_do_not_depend_on_the_rate() {
    // (the module's square wave: 256 bytes a cycle, 32 Hz at C-2, 65 Hz at C-3)
    let at_48k = crossings(48000, 2.0);
    assert!(at_48k > 40, "{at_48k}");
    for rate in [8000, 22050, 44100, 96000] {
        let n = crossings(rate, 2.0);
        assert!(n.abs_diff(at_48k) <= 2, "{n} cycles in 2 s at {rate} Hz, {at_48k} at 48000 Hz");
    }
}
