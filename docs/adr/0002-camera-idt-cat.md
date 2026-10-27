# ADR 0002: Chromatic Adaptation Transform for Camera IDTs (Bradford vs CAT02)

- **Status:** accepted (2026-10-27, approved for C2.6)
- **Context:** Managed Colour (ACES) Stage A camera IDT derivation

## Context

In an ACES color management pipeline, camera media recorded in scene-referred log spaces (Sony S-Log3, ARRI LogC3/LogC4, Canon C-Log2/C-Log3, Panasonic V-Log, Apple Log, DJI D-Log, Fujifilm F-Log) enters the ACEScg working space via an Input Device Transform (IDT).

An IDT performs:
1. Non-linear decode of camera code values to scene-linear reflectance ($18\%\text{ grey} = 0.18$).
2. A linear $3 \times 3$ matrix transformation from camera native primaries to AP1 (ACEScg) or AP0 (ACES2065-1).

All camera gamuts are defined with a standard white point (D65: $x=0.3127, y=0.3290$, or D55 for Canon daylight), whereas ACES AP0 (SMPTE ST 2065-1) and AP1 (AMPAS S-2014-004) use the ACES white point ($x=0.32168, y=0.33767$). Therefore, deriving the $3 \times 3$ transformation matrix mathematically requires a Chromatic Adaptation Transform (CAT) between the camera white point and ACES white.

Two chromatic adaptation models are prevalent in digital cinema and color science:
1. **CAT02**: Luo, Li, & R.W.G. Hunt (2002); the chromatic adaptation matrix developed for CIECAM02 and specified normatively in **AMPAS S-2014-004 §4.3**.
2. **Bradford (BFD)**: K. M. Lam (1985); the standard CAT specified in ICC.1 Annex E and commonly used in ICC workflow transforms.

We evaluated both CAT models against the external reference oracle **OpenColorIO 2.6.0** (`studio-config-v5.0.0_aces-v2.1_ocio-v2.6` in `D:\FilmCraftX-tools`) across a test grid of 17 colors (neutrals from 0.0 to 1.0 including 18% grey, Caucasian, Asian, and Dark skin tones, saturated primaries and secondaries, and synthetic out-of-gamut neon/laser colors).

## Empirical Results against OCIO 2.6 Oracle

| Camera Gamut | Bradford Max Error | Bradford Mean Error | CAT02 Max Error | CAT02 Mean Error | Closer to OCIO 2.6 |
|---|---|---|---|---|---|
| **Sony S-Gamut3.Cine** | $5.15 \times 10^{-3}$ | $1.78 \times 10^{-3}$ | **$1.52 \times 10^{-7}$** | **$3.14 \times 10^{-8}$** | **CAT02** ($\sim 34,000\times$ lower) |
| **ARRI Wide Gamut 3** | $5.08 \times 10^{-3}$ | $1.75 \times 10^{-3}$ | **$1.27 \times 10^{-7}$** | **$3.39 \times 10^{-8}$** | **CAT02** ($\sim 40,000\times$ lower) |
| **ARRI Wide Gamut 4** | $4.70 \times 10^{-3}$ | $1.62 \times 10^{-3}$ | **$6.84 \times 10^{-8}$** | **$2.62 \times 10^{-8}$** | **CAT02** ($\sim 68,000\times$ lower) |
| **Canon Cinema Gamut** | $6.11 \times 10^{-3}$ | $2.10 \times 10^{-3}$ | **$1.19 \times 10^{-7}$** | **$4.78 \times 10^{-8}$** | **CAT02** ($\sim 51,000\times$ lower) |
| **DJI D-Gamut** | $5.11 \times 10^{-3}$ | $1.77 \times 10^{-3}$ | **$1.70 \times 10^{-7}$** | **$5.32 \times 10^{-8}$** | **CAT02** ($\sim 30,000\times$ lower) |
| **Panasonic V-Gamut** | **$1.67 \times 10^{-7}$** | **$3.34 \times 10^{-8}$** | $4.62 \times 10^{-3}$ | $1.60 \times 10^{-3}$ | **Bradford** ($\sim 27,000\times$ lower) |
| **Apple (Rec. 2020)** | **$1.52 \times 10^{-7}$** | **$2.98 \times 10^{-8}$** | $3.90 \times 10^{-3}$ | $1.36 \times 10^{-3}$ | **Bradford** ($\sim 25,000\times$ lower) |
| **Fujifilm (Rec. 2020)** | **$1.52 \times 10^{-7}$** | **$2.98 \times 10^{-8}$** | $3.90 \times 10^{-3}$ | $1.36 \times 10^{-3}$ | **Bradford** ($\sim 25,000\times$ lower) |

### Key Findings:
1. **Sony, ARRI, Canon, and DJI** authored their official ACES IDT matrices strictly adhering to **AMPAS S-2014-004 §4.3**, which mandates **CAT02**. For these cameras, CAT02 matches the OCIO 2.6 oracle to machine precision ($< 2 \times 10^{-7}$), whereas Bradford introduces systematic errors exceeding $0.5\%$.
2. **Panasonic V-Gamut and ITU-R BT.2020 (Apple Log and Fujifilm F-Log)** in the Academy's reference repository (`aces-dev`) and OCIO were historically authored using **Bradford CAT** (the ICC standard). For these spaces, Bradford matches OCIO 2.6 to machine precision ($< 2 \times 10^{-7}$), whereas CAT02 introduces $\sim 0.4\%$ error.
3. Imposing a single global CAT across all cameras would inevitably create noticeable color discrepancy against external VFX facilities and NLEs conforming to either standard.

## Decision

1. **Camera-Standard Adaptive CAT:**
   FilmCraft will select the chromatic adaptation transform corresponding to the authoritative standard used by the manufacturer and Academy IDT:
   - **CAT02**: Sony (`SGamut3`, `SGamut3Cine`), ARRI (`Awg3`, `Awg4`), Canon (`CinemaGamut`), DJI (`DGamut`).
   - **Bradford**: Panasonic (`VGamut`), ITU-R BT.2020 cameras (`Apple Log`, Fujifilm `F-Log`), and general display gamuts (Rec.709, P3-D65).
2. **Pure Derivation:**
   All transformation matrices will be derived directly in pure Rust via Normalized Primary Matrix (SMPTE RP 177) combined with the selected CAT matrix, avoiding hardcoded "magic constants" and ensuring verifiable agreement with the OCIO 2.6 oracle within IEEE 754 rounding limits.
3. **No IDT Wiring Yet:**
   As instructed, this ADR establishes the decision and verifies mathematical constants prior to pipeline integration. No camera IDTs are enabled in sequence playback until C2.6 completion.
