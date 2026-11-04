// 只解释有界展示附件；字体名固定，不把候选文字或外部字符串拼进 CSS/Pango markup。
import GLib from 'gi://GLib';
import Pango from 'gi://Pango';
import PangoCairo from 'gi://PangoCairo';
const styleKey = GLib.quark_from_string('tigertype-style');
const candidateKey = GLib.quark_from_string('tigertype-candidate');
export function decode(table) {
    const style = table.get_qattachment(styleKey);
    if (!style || style.get_type_string() !== '(uuau)' || style.get_size() > 128)
        return null;
    const [version, font, colors] = style.deep_unpack();
    if (version !== 1 || ![0, 1].includes(font) || colors.length !== 8 ||
        !colors.every(c => Number.isInteger(c) && c >= 0 && c <= 0xffffff))
        return null;
    const count = table.get_number_of_candidates();
    const cursor = table.get_cursor_pos();
    if (count < 1 || count > 9 || cursor >= count || table.get_page_size() < count)
        return null;
    const rows = [];
    for (let i = 0; i < count; i++) {
        const parts = table.get_candidate(i).get_qattachment(candidateKey);
        if (!parts || parts.get_type_string() !== '(ss)' || parts.get_size() > 8192)
            return null;
        const [text, comment] = parts.deep_unpack();
        const label = table.get_label(i)?.get_text() ?? String(i + 1);
        if (!text || text.length > 1024 || comment.length > 2048 || label.length > 8)
            return null;
        rows.push({text, comment, label, selected: i === cursor && table.is_cursor_visible()});
    }
    return {font, colors, rows};
}
export function hex(color) {
    return `#${color.toString(16).padStart(6, '0')}`;
}
export function rowColors(colors, selected) {
    return selected ? [colors[5], colors[6], colors[7], colors[6]]
        : [colors[0], colors[3], colors[2], colors[4]];
}

// 绘制与离屏回归共用同一文字布局，原始文字不用 markup；长候选保留读屏全文并显示省略号。
export function shapeRow(data, font, colors, scale) {
    const [, indexInk, textInk, hintInk] = rowColors(colors, data.selected);
    const family = font === 0 ? 'LXGW WenKai GB Screen' : 'KaiTi,AR PL UKai CN,AR PL KaitiM GB';
    const context = PangoCairo.FontMap.get_default().create_context();
    const gap = 8 * scale;
    const pieces = [[data.label, 'Cascadia Mono', 13, indexInk, 40],
        [data.text, family, 20, textInk, 420],
        [data.comment, 'LXGW WenKai GB Screen', 14, hintInk, 380]]
        .filter(([text]) => text.length > 0)
        .map(([text, familyName, size, color, limit]) => {
            const layout = Pango.Layout.new(context);
            const description = new Pango.FontDescription();
            description.set_family(familyName);
            description.set_absolute_size(size * scale * Pango.SCALE);
            layout.set_font_description(description);
            // 候选是单行展示；即便收到换行也不能产生无界高度的 Shell 绘制面。
            layout.set_single_paragraph_mode(true);
            layout.set_text(text, -1);
            if (layout.get_pixel_size()[0] > limit * scale) {
                layout.set_width(limit * scale * Pango.SCALE);
                layout.set_ellipsize(Pango.EllipsizeMode.END);
            }
            const [width, height] = layout.get_pixel_size();
            return {layout, color, width, height};
        });
    return {pieces, gap, height: Math.max(28 * scale, ...pieces.map(p => p.height)),
        width: pieces.reduce((w, p) => w + p.width, 0) + gap * (pieces.length - 1)};
}

export function paintRow(cr, row) {
    let x = 0;
    for (const {layout, color, width, height} of row.pieces) {
        cr.setSourceRGBA(((color >> 16) & 255) / 255,
            ((color >> 8) & 255) / 255, (color & 255) / 255, 1);
        cr.moveTo(x, Math.round((row.height - height) / 2));
        PangoCairo.show_layout(cr, layout);
        x += width + row.gap;
    }
}
