// 获准的 GNOME 边界：只绘制虎码候选，不读文件、不联网、不轮询、不代替框架处理输入。
import St from 'gi://St';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as IBusManager from 'resource:///org/gnome/shell/misc/ibusManager.js';
import {decode, hex, rowColors, shapeRow, paintRow} from './presentation.js';

export default class TigerTypeAppearance extends Extension {
    enable() {
        this._manager = IBusManager.getIBusManager();
        this._panel = null;
        this._signals = [];
        this._saved = null;
        this._ready = this._manager.connect('ready', () => this._bind());
        this._engine = this._manager._ibus.connect('global-engine-changed', () => this._restore());
        this._bind();
    }
    disable() {
        this._unbind();
        this._manager.disconnect(this._ready);
        this._manager._ibus.disconnect(this._engine);
        this._manager = null;
    }
    _unbind() {
        this._restore();
        for (const id of this._signals) {
            // daemon 断开可能先销毁 PanelService，清理不让旧回调影响新会话。
            try { this._panel.disconnect(id); } catch (_) { /* 对象已销毁 */ }
        }
        this._signals = [];
        this._panel = null;
    }
    _bind() {
        this._unbind();
        this._panel = this._manager._panelService;
        if (!this._panel)
            return;
        this._signals.push(this._panel.connect_after('update-lookup-table', (_panel, table, visible) => {
            try {
                const model = visible && this._manager._currentEngineName === 'tigertype' ? decode(table) : null;
                if (model)
                    this._paint(model);
                else
                    this._restore();
            } catch (error) {
                // 失败还原系统面板，不破坏输入链路，不输出候选或用户文字。
                this._restore();
                console.error(`TigerType 候选外观适配失败：${error.message}`);
            }
        }));
        this._signals.push(this._panel.connect('focus-out', () => this._restore()));
    }
    _save(popup) {
        const area = popup._candidateArea;
        const content = popup.bin.get_child();
        if (!Array.isArray(area?._candidateBoxes) || area._candidateBoxes.length < 9 || !content)
            throw new Error('不支持的 GNOME 候选结构');
        const actors = [popup, content, popup._preeditText, popup._auxText,
            area._previousButton, area._nextButton, ...area._candidateBoxes];
        const styles = actors.map(actor => [actor, actor.get_style()]);
        const rows = area._candidateBoxes.map(box => {
            const drawing = new St.DrawingArea();
            const row = {box, drawing, indexVisible: box._indexLabel.visible,
                textVisible: box._candidateLabel.visible, name: box.accessible_name, pieces: []};
            drawing.connect('repaint', () => {
                const cr = drawing.get_context();
                let failed = false;
                try {
                    // Pango/Cairo 按字形绘制，避开 Clutter 对整份含 COLR 字体的彩色纹理判断。
                    paintRow(cr, row);
                } catch (_) {
                    failed = true;
                } finally { cr.$dispose(); }
                if (failed)
                    this._restore();
            });
            box.add_child(drawing);
            return row;
        });
        this._saved = {popup, content, area, styles, rows};
    }
    _paint(model) {
        const popup = this._manager._candidatePopup;
        if (this._saved?.popup !== popup) {
            this._restore();
            this._save(popup);
        }
        const {content, area, rows} = this._saved;
        const c = model.colors;
        const scale = St.ThemeContext.get_for_stage(global.stage).scale_factor;
        popup.set_style(`-arrow-background-color: ${hex(c[0])}; -arrow-border-color: ${hex(c[1])}; -arrow-border-width: 1px; -arrow-border-radius: 8px; color: ${hex(c[2])};`);
        content.set_style(`background-color: ${hex(c[0])}; color: ${hex(c[2])}; padding: 6px; border-radius: 8px;`);
        for (const label of [popup._preeditText, popup._auxText])
            label.set_style(`color: ${hex(c[2])}; font-family: "Cascadia Mono"; font-size: 13px;`);
        for (const button of [area._previousButton, area._nextButton])
            button.set_style(`color: ${hex(c[2])}; background-color: ${hex(c[0])}; border-color: ${hex(c[1])}; box-shadow: none;`);
        for (let i = 0; i < rows.length; i++) {
            const row = rows[i];
            const data = model.rows[i];
            row.drawing.visible = Boolean(data);
            if (!data)
                continue;
            const [background, , textInk] = rowColors(c, data.selected);
            row.box.set_style(`background-color: ${hex(background)}; color: ${hex(textInk)}; border: none; border-radius: 6px; box-shadow: none; padding: 4px 8px;`);
            row.box._indexLabel.hide();
            row.box._candidateLabel.hide();
            // 保留原生候选行及点击行为，读屏仍取得完整字词与注释。
            row.box.accessible_name = `${data.label} ${data.text} ${data.comment}`;
            Object.assign(row, shapeRow(data, model.font, c, scale));
            row.drawing.set_size(row.width, row.height);
            row.drawing.queue_repaint();
        }
    }
    _restore() {
        if (!this._saved)
            return;
        const saved = this._saved;
        this._saved = null;
        for (const row of saved.rows) {
            row.box._indexLabel.visible = row.indexVisible;
            row.box._candidateLabel.visible = row.textVisible;
            row.box.accessible_name = row.name;
            row.drawing.destroy();
        }
        for (const [actor, style] of saved.styles)
            actor.set_style(style);
    }
}
