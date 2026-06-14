"""Unicode, emoji, and text rendering tests.

Tests UTF-8 encoding, CJK wide characters, combining characters,
emoji (including ZWJ sequences, skin tones, flags), bidirectional
text, and programming ligatures.
"""

from ..terminal import Terminal
from ..reporter import (
    TestResultOrStatus,
    TestCase,
    heading,
    subheading,
    info,
    prompt_visual,
    register,
)


def _test_utf8(term: Terminal) -> TestResultOrStatus:
    """Test basic UTF-8 encoding correctness."""
    heading(term, "UTF-8 Encoding")

    # Basic ASCII
    subheading(term, "ASCII (1-byte)")
    term.write(b"  !\"#$%%&'()*+,-./0123456789:;<=>?@\n")
    term.write(b"  ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`\n")
    term.write(b"  abcdefghijklmnopqrstuvwxyz{|}~\n\n")

    # Latin-1 Supplement (2-byte)
    subheading(term, "Latin-1 Supplement (U+0080-U+00FF)")
    term.write("  ©®¡¢£¤¥¦§¨©ª«¬\u00ad®¯°±²³´µ¶·¸¹º»¼½¾¿\n".encode("utf-8"))
    term.write("  ÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖ×ØÙÚÛÜÝÞßàáâãäåæç\n".encode("utf-8"))
    term.write("  èéêëìíîïðñòóôõö÷øùúûüýþÿ\n\n".encode("utf-8"))

    # CJK (3-byte) — 汉字
    subheading(term, "CJK Unified Ideographs (U+4E00-U+9FFF)")
    term.write("  的一是不了人我在有他这为之大来以个中上们说到时\n".encode("utf-8"))
    term.write("  汉字测试：上下左右中，天地人水火风\n\n".encode("utf-8"))

    # Beyond BMP (4-byte) — 𠀀 etc
    subheading(term, "Supplementary Plane (4-byte)")
    term.write("  𠀀 𠀁 𠀂 𡀀 𡀁 𡀂 𢀀 𢀁 𢀂 𣀀 𣀁 𣀂\n\n".encode("utf-8"))

    # Invalid sequences
    subheading(term, "Invalid / Overlong Sequences (should show replacement)")
    term.write(b"  Invalid: \x80\x81\x82\x83  ")
    term.write(b"  Overlong: \xc0\x80 \xe0\x80\x80 \xf0\x80\x80\x80\n")
    term.write(b"  (Should display as \ufffd replacement characters)\n\n")

    # Ruler to check alignment
    subheading(term, "Alignment Ruler")
    for i in range(8):
        term.write(f"{i * 10:10d}".encode())
    term.write(b"\n")
    term.write(b"0123456789" * 8 + b"\n")

    # CJK alignment check
    term.write("汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字".encode("utf-8"))
    term.write(b"\n")
    term.write(b"....+....|....+....|....+....|....+....|....+....|....+....|\n")
    term.flush()

    return prompt_visual(
        term,
        "All UTF-8 sequences displayed correctly? CJK double-width? "
        "Invalid sequences show \ufffd?",
    )


def _test_cjk(term: Terminal) -> TestResultOrStatus:
    """Test CJK (Chinese/Japanese/Korean) character rendering."""
    heading(term, "CJK Character Rendering")

    # Chinese
    subheading(term, "中文 Chinese")
    term.write("  床前明月光，疑是地上霜。\n".encode("utf-8"))
    term.write("  举头望明月，低头思故乡。\n\n".encode("utf-8"))

    # Japanese
    subheading(term, "日本語 Japanese")
    term.write("  春はあけぼの。やうやう白くなりゆく山際、\n".encode("utf-8"))
    term.write("  少し明かりて、紫だちたる雲の細くたなびきたる。\n\n".encode("utf-8"))

    # Korean
    subheading(term, "한국어 Korean")
    term.write("  대한민국의 국민은 민족문화의 전통을 계승하고\n".encode("utf-8"))
    term.write("  민주주의 바탕 위에 통일을 지향한다.\n\n".encode("utf-8"))

    # Mixed CJK + Latin
    subheading(term, "Mixed CJK + Latin")
    term.write("  Hello 世界！今日は いい天気ですね。\n".encode("utf-8"))
    term.write("  안녕하세요, nice to meet you! 你好\n\n".encode("utf-8"))

    # CJK + Latin alignment test
    subheading(term, "CJK/Latin Alignment (each line should be same total width)")
    term.write(b"  ....+....|....+....|....+....|....+....|....+....|\n")
    term.write("  汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字\n".encode("utf-8"))
    term.write(b"  aabbccddeeffgghhiijjkkllmmnnooppqqrrssttuu\n")
    term.write("  漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字\n".encode("utf-8"))
    term.write(b"  0123456789012345678901234567890123456789012345\n")
    term.write(b"\n")

    # Ruby/annotation check
    info(term, "Check: CJK characters should occupy exactly 2 columns each.")
    info(term, 'The "汉字" lines should span the same width as the Latin lines.')

    return prompt_visual(
        term, "CJK characters rendered at correct width? Mixed text aligned?"
    )


def _test_combining(term: Terminal) -> TestResultOrStatus:
    """Test combining characters and grapheme clusters."""
    heading(term, "Combining Characters & Grapheme Clusters")

    # Latin with combining diacritics
    subheading(term, "Latin + Combining Diacritics")
    # Precomposed vs decomposed
    term.write(b"  Precomposed:  \xc3\xa9 \xc3\xa0 \xc3\xbc \xc3\xb1 \xc5\xa1\n")
    term.write(b"  Decomposed:   e\xcc\x81 a\xcc\x80 u\xcc\x88 n\xcc\x83 s\xcc\x8c\n")
    term.write(b"  (Both should look the same)\n\n")

    # Stacking diacritics
    subheading(term, "Stacking Diacritics")
    term.write("  Z̴̧̛̘̲̮̺̖̙̼͎̦̮̳̹͈ą̴̡̢̛̛̛̛̛̛̝̝̲̗̠͇̯͔͎l̸̨̢̛̛̛̘̮̺̖̙̼͎̦̮̳̹͈ģ̷̛̘̲̮̺̖̙̼͎̦̮̳̹͈ǫ̶̡̢̛̛̛̛̛̛̝̝̲̗̠͇̯͔͎!\n".encode("utf-8"))

    # Devanagari
    subheading(term, "Devanagari (Hindi)")
    term.write("  नमस्ते दुनिया! सभी मनुष्यों को गरिमा और\n".encode("utf-8"))
    term.write("  अधिकारों में समानता प्राप्त है।\n\n".encode("utf-8"))

    # Thai
    subheading(term, "Thai")
    term.write("  สวัสดีชาวโลก! มนุษย์ทุกคนเกิดมามีอิสระ\n".encode("utf-8"))
    term.write("  และเสมอภาคกันในศักดิ์ศรีและสิทธิ\n\n".encode("utf-8"))

    # Arabic
    subheading(term, "العربية (Arabic)")
    term.write("  السلام عليكم ورحمة الله وبركاته\n".encode("utf-8"))
    term.write(
        "  جميع الناس يولدون أحرارًا متساوين في الكرامة والحقوق\n\n".encode("utf-8")
    )

    # Mixed RTL/LTR
    subheading(term, "Bidirectional (Arabic + English)")
    term.write("  مرحبا world! هذا test 123 مع mixing.\n".encode("utf-8"))
    term.write("  الساعة 12:30 مساءً يوم Monday\n\n".encode("utf-8"))

    return prompt_visual(
        term,
        "Combining characters render correctly (precomposed=decomposed)?\n"
        "  Arabic renders right-to-left? Devanagari/Thai shaped correctly?",
    )


def _test_emoji(term: Terminal) -> TestResultOrStatus:
    """Test emoji rendering including ZWJ, skin tones, flags."""
    heading(term, "Emoji Rendering")

    # Basic emoji
    subheading(term, "Basic Emoji (U+1F600+)")
    term.write(b"  Faces:   ")
    term.write("😀😃😄😁😆😅🤣😂🙂🙃😉😊😇🥰😍🤩😘😗😚😋😛😜🤪😝🤑🤗\n".encode("utf-8"))
    term.write(b"  Animals: ")
    term.write("🐶🐱🐭🐹🐰🦊🐻🐼🐨🐯🦁🐮🐷🐸🐵🐔🐧🐦🐤🐣🐥🦆🦅🦉🦇🐺\n".encode("utf-8"))
    term.write(b"  Food:    ")
    term.write("🍏🍎🍐🍊🍋🍌🍉🍇🍓🫐🍈🍒🍑🥭🍍🥥🥝🍅🍆🥑🥦🥬🥒🌶🫑🌽\n".encode("utf-8"))
    term.write(b"  Hearts:  ")
    term.write("❤️🧡💛💚💙💜🖤🤍🤎💔❣️💕💞💓💗💖💘💝\n\n".encode("utf-8"))

    # Skin tones
    subheading(term, "Skin Tone Modifiers")

    # Basic smiley hand wave with different skin tones
    hand = "👋"
    tones = ["🏻", "🏼", "🏽", "🏾", "🏿"]
    term.write(b"  Hand wave: ")
    for tone in tones:
        term.write(f"{hand}{tone} ".encode("utf-8"))
    term.write(b"\n")

    # Thumbs up
    thumb = "👍"
    term.write(b"  Thumbs up: ")
    for tone in tones:
        term.write(f"{thumb}{tone} ".encode("utf-8"))
    term.write(b"\n\n")

    # ZWJ sequences
    subheading(term, "ZWJ (Zero-Width Joiner) Sequences")

    term.write(b"  Family:     ")
    term.write("👨‍👩‍👧‍👦\n".encode("utf-8"))
    term.write(b"  Couple:     ")
    term.write("👩‍❤️‍👨\n".encode("utf-8"))
    term.write(b"  Kiss:       ")
    term.write("💏\n".encode("utf-8"))
    term.write(b"  Profession: ")
    term.write("👨‍💻 👩‍🔬 🧑‍🌾 👨‍🎓 👩‍🚀 🧑‍🏫\n".encode("utf-8"))
    term.write(b"  Family (variants): ")
    term.write("👨‍👩‍👦 👨‍👩‍👧 👨‍👩‍👧‍👦 👨‍👩‍👦‍👦 👨‍👩‍👧‍👧\n".encode("utf-8"))
    term.write(b"  Heart:      ")
    term.write("👩‍❤️‍👩 👨‍❤️‍👨 👩‍❤️‍💋‍👨 👨‍❤️‍💋‍👨\n".encode("utf-8"))

    # Flags
    subheading(term, "Flags (Regional Indicators)")
    term.write(b"  Countries: ")
    term.write("🇯🇵 🇰🇷 🇨🇳 🇺🇸 🇬🇧 🇫🇷 🇩🇪 🇮🇹 🇪🇸 🇵🇹 🇧🇷 🇷🇺 🇮🇳\n".encode("utf-8"))
    term.write(b"  More:      ")
    term.write("🇦🇺 🇨🇦 🇲🇽 🇦🇷 🇸🇪 🇳🇴 🇫🇮 🇩🇰 🇳🇱 🇧🇪 🇨🇭 🇦🇹 🇮🇱\n".encode("utf-8"))

    # Keycap sequences
    subheading(term, "Keycap Sequences")
    term.write(b"  Numbers:   ")
    term.write("0️⃣ 1️⃣ 2️⃣ 3️⃣ 4️⃣ 5️⃣ 6️⃣ 7️⃣ 8️⃣ 9️⃣  🔟\n".encode("utf-8"))

    # Special sequences
    subheading(term, "Special Sequences")
    term.write(b"  Rainbow:   ")
    term.write("🏳️‍🌈\n".encode("utf-8"))
    term.write(b"  Pirate:    ")
    term.write("🏴‍☠️\n".encode("utf-8"))
    term.write(b"  Trans:     ")
    term.write("🏳️‍⚧️\n".encode("utf-8"))

    info(
        term,
        "Expected: ZWJ sequences should join into single emoji (not separate glyphs).",
    )
    info(term, "Flags should show as country flags, not letter pairs.")
    info(term, "Skin tones should apply to the base emoji.")

    return prompt_visual(
        term,
        "Emoji rendered correctly? ZWJ sequences joined? "
        "Skin tones applied? Flags display properly?",
    )


def _test_ligatures(term: Terminal) -> TestResultOrStatus:
    """Test programming ligatures (font-dependent)."""
    heading(term, "Programming Ligatures")

    info(term, "Note: This test depends on your terminal font.")
    info(
        term,
        "Ligatures require a font with them (e.g., Fira Code, JetBrains Mono, Cascadia Code).",
    )

    subheading(term, "Arrows")
    term.write(b"  ->  ->  <-  <=>  =>  ==>  <==\n")
    term.write(b"  <|  ||  |>  <||  |||>  <|>\n\n")

    subheading(term, "Comparison / Equality")
    term.write(b"  ==  ===  !=  !==  <=  >=  <>\n\n")

    subheading(term, "Assignment")
    term.write(b"  :=  ::  :>=  :<=  :>  :<\n\n")

    subheading(term, "Arithmetic / Logic")
    term.write(b"  >>  <<  ++  --  &&  ||  //  /* */\n\n")

    subheading(term, "Fancy")
    term.write(b"  ===  !==  ==>  <==  <=>  >>=  <<=\n")
    term.write(b"  |=>  ||=>  ~@  ..  ...  ..<\n\n")

    subheading(term, "Rust-specific")
    term.write(b"  ->  =>  ::  ..  ..=  ...\n\n")

    subheading(term, "Arrow combinations")
    term.write(b"  --->>  -->>  --<  -->  <---  <-->  <===>\n")
    term.write(b"  |>  <|  |]  [|  {|  |}\n\n")

    subheading(term, "No-ligature reference (same text, spaced)")
    term.write(b"  - >  = >  < =  = =  ! =  : =  | >  < |\n")
    term.write(b"  (Characters above should appear merged in ligature font)\n")

    info(
        term, "Ligature fonts: Fira Code, JetBrains Mono, Cascadia Code, Iosevka, etc."
    )
    info(term, "If all characters appear separate, your font lacks ligature support.")

    return prompt_visual(
        term, "Ligatures visible (glyphs merged)? Or all characters separate?"
    )


def _test_powerline(term: Terminal) -> TestResultOrStatus:
    """Test Powerline and Nerd Font glyph rendering."""
    heading(term, "Powerline & Nerd Font Glyphs")

    info(term, "Note: This test requires a font with Powerline/Nerd Font patches.")
    info(term, "Common fonts: FiraCode Nerd Font, JetBrainsMono Nerd Font,")
    info(term, "Meslo Nerd Font, SourceCodePro Powerline, etc.")

    # Powerline symbols
    subheading(term, "Powerline Symbols (U+E0A0–E0C8)")

    powerline_glyphs = [
        # Arrows / triangles
        ("Branch", "\ue0a0\ue0a1\ue0a2\ue0a3"),
        ("Line numbers", "\ue0a1"),
        ("Triangle L/R", "\ue0b0\ue0b2"),
        ("Triangle R/L (soft)", "\ue0b1\ue0b3"),
        # Others
        ("Flame", "\ue0b6"),
        ("Heart", "\ue0b7"),
        ("Star/Sun", "\ue0b8"),
        ("Padlock", "\ue0b9"),
    ]

    for name, glyphs in powerline_glyphs:
        term.write(f"  {name:20s}: {glyphs}\n".encode("utf-8"))
    term.flush()

    # Nerd Font symbols
    subheading(term, "Nerd Font Icons (selected)")
    nerd_font = [
        ("File icon", "\uf15b"),  # 
        ("Folder", "\uf114"),  # 
        ("Terminal", "\uf489"),  # 
        ("Code", "\ue795"),  #  → actually 7xx range
        ("Git", "\ue702"),  # 
        ("Git branch", "\ue725"),  # 
        ("Git merge", "\ue727"),  # 
        ("Home", "\uf015"),  # 
        ("Settings", "\uf013"),  # 
        ("CPU", "\uf266"),  # 
        ("Download", "\uf019"),  # 
        ("Cloud", "\uf0c2"),  # 
        ("Package", "\uf1fe"),  # 
        ("Lock", "\uf023"),  # 
        ("Key", "\uf084"),  # 
        ("User", "\uf007"),  # 
    ]

    for name, glyph in nerd_font:
        term.write(f"  {name:20s}: {glyph}\n".encode("utf-8"))
    term.flush()

    # Devicons
    subheading(term, "Devicons (file type icons)")
    devicons = [
        ("JS", "\ue781"),
        ("Py", "\ue73c"),
        ("Java", "\ue738"),
        ("Ruby", "\ue739"),
        ("Go", "\ue724"),
        ("Rust", "\ue7a8"),
        ("HTML", "\ue7a5"),
        ("CSS", "\ue7a2"),
        ("Docker", "\ue7b0"),
    ]

    for name, glyph in devicons:
        term.write(f"  {name:10s}: {glyph}\n".encode("utf-8"))
    term.flush()

    # Font Awesome
    subheading(term, "Font Awesome Brand Icons")
    brands = [
        ("GitHub", "\uf09b"),
        ("Twitter", "\uf099"),
        ("Docker", "\uf21b"),
        ("Apple", "\uf179"),
        ("Linux", "\uf17c"),
        ("Windows", "\uf17a"),
        ("Rust", "\ue7a8"),
    ]

    for name, glyph in brands:
        term.write(f"  {name:15s}: {glyph}\n".encode("utf-8"))
    term.flush()

    term.write(b"\n")

    info(term, "Expected: Powerline triangles/arrows in correct direction.")
    info(term, "Nerd Font/Devicons: each shows a distinct icon.")
    info(term, "Missing glyphs show as hex boxes or fallback characters.")

    return prompt_visual(term, "Powerline/Nerd Font glyphs rendered correctly?")


def register_unicode_tests():
    register(
        TestCase(
            "unicode-utf8",
            "unicode",
            "UTF-8 Encoding",
            "ASCII, Latin-1, CJK, Supplementary, invalid sequences",
            _test_utf8,
            auto_verify=True,
        )
    )
    register(
        TestCase(
            "unicode-cjk",
            "unicode",
            "CJK Characters",
            "Chinese, Japanese, Korean, mixed Latin/CJK",
            _test_cjk,
        )
    )
    register(
        TestCase(
            "unicode-combining",
            "unicode",
            "Combining Characters & Bidi",
            "Diacritics, Devanagari, Thai, Arabic, RTL/LTR",
            _test_combining,
        )
    )
    register(
        TestCase(
            "unicode-emoji",
            "unicode",
            "Emoji & ZWJ Sequences",
            "Basic emoji, skin tones, ZWJ, flags, keycaps",
            _test_emoji,
        )
    )
    register(
        TestCase(
            "unicode-ligatures",
            "unicode",
            "Programming Ligatures",
            "Arrows, comparisons, assignment, Rust-specific ligatures",
            _test_ligatures,
        )
    )
    register(
        TestCase(
            "unicode-powerline",
            "unicode",
            "Powerline & Nerd Font Glyphs",
            "Powerline triangles, Nerd Font icons, Devicons, Font Awesome brands",
            _test_powerline,
        )
    )
