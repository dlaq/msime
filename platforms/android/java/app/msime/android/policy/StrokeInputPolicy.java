package app.msime.android;

/**
 * 宿主怎样驱动笔画方案：Engine 读 `msime-stroke.db`，按横竖撇点折（h s p n z，x 为通配）的笔顺给出字词候选；空格或撇号分隔词组中的字。
 *
 * <p>The editor belongs to the Engine: this host sends the stroke letters, marks the view's `reading` (the typed strokes drawn as 一丨丿丶乛＊) inline rather than its `editing_text` (the letters), and inserts whatever the Engine commits (the Stroke contract in msime_client.h). Candidates are on the strip as in pinyin, they are read-only, and nothing is learned.
 */
public final class StrokeInputPolicy {
    /** `SchemeType::Stroke`: the value `View.scheme` and `commit_context.scheme` carry for this scheme. */
    public static final int STROKE_SCHEME = InputSchemeTraits.STROKE;

    private StrokeInputPolicy() {}

    /** Whether the Engine's stroke editor takes the keys and its reading is what the editor marks: the Stroke scheme outside dedicated English. Stroke has no local modes. */
    public static boolean active(int scheme, boolean dedicatedEnglish) {
        return InputSchemeTraits.active(scheme, STROKE_SCHEME, dedicatedEnglish);
    }

    /** Whether an ASCII key (such as {@code '\''} or {@code ' '}) is currently claimed by the Engine as a stroke phrase separator via {@code View.spelling_symbols}. */
    public static boolean isEngineKey(boolean stroke, int unicode, String spellingSymbols) {
        if (!stroke || unicode < 32 || unicode > 126 || spellingSymbols == null) return false;
        return spellingSymbols.indexOf((char) unicode) >= 0;
    }

    /** Whether touch Space inserts the stroke phrase separator instead of committing/inserting a literal space. */
    public static boolean spaceIsEngineKey(boolean stroke, String spellingSymbols) {
        return isEngineKey(stroke, ' ', spellingSymbols);
    }
}
