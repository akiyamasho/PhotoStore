# CapCut 9.5.0 reference catalogue for PhotoStore issue #23

## Evidence and limits

Recorded 2026-10-27 from **the installed CapCut 9.5.0 for macOS**, Help → Shortcut, selected profile **Shortcut 1**. The version was read from the app bundle's public Info.plist. The four visible categories were inspected; the timeline list was scrolled to its bottom. The dialog was closed with Cancel; no shortcut settings or project contents were changed. These are functional UI observations, not extracted application code.

The user's supplied reference image is `codex-clipboard-56eb2ea5-329f-4d38-a408-9b8b8b339b67.png` (2662×1470). Its artwork/project details are not repository assets. The screenshot establishes layout; it cannot establish hidden keyframe states or shortcuts.

**Do not label this profile as universal CapCut defaults.** Its Previous/Next frame and Shuttle left bindings differ from common published conventions. The displayed many-frame direction labels also appear reversed relative to the arrow glyphs. Record them literally below; PhotoStore should provide intuitive Left/Right frame defaults and J/K/L shuttle, with this observed profile's non-conflicting aliases available in the editable compatibility preset. The label `In` is ambiguous: preserve the observation, map it to entering selected compound contents only as an explicitly named PhotoStore adaptation, not a verified CapCut semantic claim. `Reset` is displayed with Cmd+Shift+Z; PhotoStore labels its corresponding history action Redo.

`Unassigned` means the source catalogue lists an action but shows no key. It does not establish that PhotoStore implements that feature. Key-only entries and mouse/held-modifier gestures are accounted for separately in implementation. On other platforms `Cmd` is the primary platform modifier; do not claim the complete Windows bindings were inspected.

## Timeline (81 entries)

| Native action label | Observed macOS binding |
| --- | --- |
| Split | `Cmd+B` |
| Split all | `Cmd+Shift+B` |
| Select mode | `A` |
| Split mode | `B` |
| Select leftward | `[` |
| Select rightward | `]` |
| Main track magnet | `P` |
| Auto snapping | `N` |
| Linkage switch | `~` |
| Preview axis switch | `S` |
| Zoom in | `Cmd++` |
| Zoom out | `Cmd+-` |
| Zoom in/Zoom out | `Cmd+Scroll` |
| Up/Down | `Scroll` |
| Left/Right | `Alt+Scroll` |
| Activate/Deactivate clip | `V` |
| Extract/Restore audio | `Ctrl+Shift+S` |
| Add marker | `M` |
| Add marker with new color | `Alt+M` |
| Go to next marker | `Shift+M` |
| Go to previous marker | `Alt+Shift+M` |
| Previous frame | `Cmd+Shift+W` |
| Next frame | `Cmd+Shift+E` |
| Locate to first frame | `Home` |
| Locate to last frame | `End` |
| Last cut point | `Up` |
| Next cut point | `Down` |
| In point | `I` |
| Out point | `O` |
| Select by clip | `Shift+X` |
| Deselect | `Alt+X` |
| Group | `Cmd+G` |
| Ungroup | `Cmd+Shift+G` |
| Show speed panel | `Cmd+R` |
| Curve speed | `Shift+B` |
| Create compound clip (subproject) | `Alt+G` |
| Undo compound clip (subproject) | `Alt+Shift+G` |
| Volume +0.1dB | `Cmd+.` |
| Volume -0.1dB | `Cmd+,` |
| In | `Shift+Enter` |
| Shuttle left | `Cmd+P` |
| Shuttle stop | `K` |
| Shuttle right | `L` |
| Delete left | `Q` |
| Delete right | `W` |
| Show/Hide keyframe panel | `Alt+K` |
| Add keyframe on keyframe panel | `Shift+Click` |
| Add keyframe | `Alt+Shift+K` |
| Ease in | Unassigned |
| Ease out | Unassigned |
| Easing | Unassigned |
| Linear | Unassigned |
| Adjust properties | Unassigned |
| Step forward many frames | `Shift+Left` |
| Step back many frames | `Shift+Right` |
| Zoom to fit timeline | `Shift+Z` |
| Freeze | Unassigned |
| Reverse | Unassigned |
| Mirror | Unassigned |
| Rotate | Unassigned |
| Crop | Unassigned |
| Trim clip | Unassigned |
| Generate captions | Unassigned |
| Don’t isolate voice | Unassigned |
| Keep vocal | Unassigned |
| Remove vocal | Unassigned |
| Sync video and audio | Unassigned |
| Transcript | Unassigned |
| Open file location | Unassigned |
| Link to media | Unassigned |
| Separate | Unassigned |
| Split scenes | Unassigned |
| Replace clip | Unassigned |
| Image to video | Unassigned |
| Export selected clips | Unassigned |
| Apply as a LIVE photo | Unassigned |
| Create multi-camera clip | Unassigned |
| Save preset | Unassigned |
| Pre-process compound clip (subproject) | Unassigned |
| Clear beats | Unassigned |
| Voiceover | Unassigned |

## Player (8 entries)

| Native action label | Observed macOS binding |
| --- | --- |
| Play/Pause | `Space` |
| Enter/Exit full screen | `Cmd+Shift+F` |
| Cancel player alignment | `Hold Cmd` |
| Zoom in player | `Alt+Shift++` |
| Zoom out player | `Alt+Shift+-` |
| Zoom to fit player | `Alt+Shift+Z` |
| Show/hide rulers | `Cmd+Alt+R` |
| Show/hide guides | `Cmd+;` |

## Basic (17 entries)

| Native action label | Observed macOS binding |
| --- | --- |
| Copy | `Cmd+C` |
| Cut | `Cmd+X` |
| Paste | `Cmd+V` |
| Copy attributes | `Cmd+Shift+C` |
| Paste attributes | `Cmd+Shift+V` |
| Delete | `Backspace` |
| Undo | `Cmd+Z` |
| Reset | `Cmd+Shift+Z` |
| Import media | `Cmd+I` |
| Export still frames | Unassigned |
| Export | `Cmd+E` |
| New project | `Cmd+N` |
| Switch material panel | `Tab` |
| Back to edit | `Cmd+Alt+Q` |
| Quit | `Cmd+Q` |
| Full screen | `Cmd+Ctrl+F` |
| Exit full screen | `Escape` |

## Other (1 entries)

| Native action label | Observed macOS binding |
| --- | --- |
| Resize the text box from the center | `Alt+Drag` |

## Public primary references

- [CapCut desktop keyframe guide](https://www.capcut.com/resource/how-to-add-keyframes-in-capcut): inspector property diamonds, moving keys, Alt+K and Shift+Click; the app catalogue additionally shows Alt+Shift+K.
- [CapCut keyframe animation](https://www.capcut.com/tools/keyframe-animation): property animation, auto-creation after an initial key, and motion refinement with graph/ease controls.
- [CapCut split workflow](https://www.capcut.com/features/how-to-split-a-video-capcut-workflow-comparison): split selected footage at the playhead; the article documents Ctrl+B, while the inspected Mac UI shows Cmd+B.
- [Adobe timeline animation workflow](https://helpx.adobe.com/photoshop/desktop/add-video-and-animation/use-keyframes/create-timeline-animation-workflow.html): layer property animation and automatic keys after stopwatch activation.
- [Adobe animated poster tutorial](https://www.adobe.com/learn/photoshop/web/animate-poster-text-images): editable Smart Objects and Transform animation.

The UI layout, shortcut observations, documented behavior, and PhotoStore design choices must remain distinguished. No CapCut code, branded assets, project artwork, or user shortcut files are copied into PhotoStore.
