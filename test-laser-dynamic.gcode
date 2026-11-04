(Dynamic laser power test pattern - simulation/test material only)
(Verify enclosure, focus, material, and fire safety before any physical run.)
(Each horizontal line uses a different S power value; M4 enables dynamic power.)
G21
G90
G94
G17
G54

(Keep output off during positioning.)
M5
G0 X20 Y20 Z0

(Low power)
S32
F1200
M4
G1 X260 Y20
M5

(Light mark)
G0 X260 Y40
S64
F1200
M4
G1 X20 Y40
M5

(Medium-light mark)
G0 X20 Y60
S96
F1200
M4
G1 X260 Y60
M5

(Medium mark)
G0 X260 Y80
S128
F1200
M4
G1 X20 Y80
M5

(Deep mark)
G0 X20 Y100
S192
F1200
M4
G1 X260 Y100
M5

(Highest test level)
G0 X260 Y120
S255
F1200
M4
G1 X20 Y120
M5

(Repeat one power level at slower feed to demonstrate dynamic feed compensation.)
G0 X20 Y140
S128
F600
M4
G1 X260 Y140
M5

G0 X20 Y20
M30
