// Interface clicks, synthesized with WebAudio so no sound files ship. Kept
// short and quiet: they confirm input, they shouldn't be noticed otherwise.

type Sound = "move" | "select" | "back" | "error";

let enabled = true;
let ctx: AudioContext | null = null;
let lastMove = 0;

export function setSoundsEnabled(on: boolean) {
  enabled = on;
}

const VOICES: Record<Sound, { from: number; to: number; ms: number; gain: number; type: OscillatorType }> = {
  move: { from: 1250, to: 1150, ms: 28, gain: 0.025, type: "sine" },
  select: { from: 700, to: 1050, ms: 70, gain: 0.05, type: "triangle" },
  back: { from: 620, to: 420, ms: 70, gain: 0.045, type: "triangle" },
  error: { from: 220, to: 180, ms: 120, gain: 0.05, type: "square" },
};

export function play(sound: Sound) {
  if (!enabled) return;
  const now = performance.now();
  // Key repeat would otherwise turn into a buzz.
  if (sound === "move" && now - lastMove < 45) return;
  if (sound === "move") lastMove = now;
  try {
    ctx ??= new AudioContext();
    if (ctx.state === "suspended") void ctx.resume();
    const v = VOICES[sound];
    const t = ctx.currentTime;
    const osc = ctx.createOscillator();
    const amp = ctx.createGain();
    osc.type = v.type;
    osc.frequency.setValueAtTime(v.from, t);
    osc.frequency.exponentialRampToValueAtTime(v.to, t + v.ms / 1000);
    amp.gain.setValueAtTime(v.gain, t);
    amp.gain.exponentialRampToValueAtTime(0.0001, t + v.ms / 1000);
    osc.connect(amp).connect(ctx.destination);
    osc.start(t);
    osc.stop(t + v.ms / 1000 + 0.02);
  } catch {
    // No audio device: stay silent.
  }
}
