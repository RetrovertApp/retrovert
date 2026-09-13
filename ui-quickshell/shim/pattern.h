#pragma once

#include <QColor>
#include <QFont>
#include <QQuickItem>
#include <QtQml/qqmlregistration.h>
#include <vector>

#include "retrovert_ui.h"
#include "session.h"

// The tracker pattern grid: the rows around the playing row, one channel per column, drawn
// straight into the scene graph as glyphs. The bridge publishes the decoder's row window as
// classed cells; this item asks for the rows it can show, centres the playing row, and
// rebuilds its text only when that row or the window changes, never per frame.
//
// Columns per channel are the leading ones that fit the channel's width: a narrow channel
// shows note and instrument, a wide one every column the decoder has. `numberWidth` is the
// row-number gutter on the left; the channel width is what remains divided evenly, which is
// what a header row of scopes above the channels lays itself out from.
class Pattern : public QQuickItem {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(Session* session READ session WRITE setSession NOTIFY sessionChanged)
    Q_PROPERTY(QFont font READ font WRITE setFont NOTIFY fontChanged)
    Q_PROPERTY(qreal rowHeight READ rowHeight WRITE setRowHeight NOTIFY layoutChanged)
    Q_PROPERTY(qreal numberWidth READ numberWidth WRITE setNumberWidth NOTIFY layoutChanged)
    Q_PROPERTY(qreal numberPadding READ numberPadding WRITE setNumberPadding NOTIFY layoutChanged)
    // How far the rows farthest from the cursor fade, 0 for none and 1 for invisible.
    Q_PROPERTY(qreal fade READ fade WRITE setFade NOTIFY layoutChanged)
    Q_PROPERTY(QColor noteColor READ noteColor WRITE setNoteColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor noteOffColor READ noteOffColor WRITE setNoteOffColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor instrumentColor READ instrumentColor WRITE setInstrumentColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor volumeColor READ volumeColor WRITE setVolumeColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor effectColor READ effectColor WRITE setEffectColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor paramColor READ paramColor WRITE setParamColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor emptyColor READ emptyColor WRITE setEmptyColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor numberColor READ numberColor WRITE setNumberColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor cursorColor READ cursorColor WRITE setCursorColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor cursorLineColor READ cursorLineColor WRITE setCursorLineColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor lineColor READ lineColor WRITE setLineColor NOTIFY colorsChanged)

public:
    explicit Pattern(QQuickItem* parent = nullptr);

    Session* session() const { return m_session; }
    void setSession(Session* session);
    QFont font() const { return m_font; }
    void setFont(const QFont& font);
    qreal rowHeight() const { return m_rowHeight; }
    void setRowHeight(qreal height);
    qreal numberWidth() const { return m_numberWidth; }
    void setNumberWidth(qreal width);
    qreal numberPadding() const { return m_numberPadding; }
    void setNumberPadding(qreal padding);
    qreal fade() const { return m_fade; }
    void setFade(qreal fade);

    QColor noteColor() const { return m_colors[kNote]; }
    void setNoteColor(const QColor& c) { setColorAt(kNote, c); }
    QColor noteOffColor() const { return m_colors[kNoteOff]; }
    void setNoteOffColor(const QColor& c) { setColorAt(kNoteOff, c); }
    QColor instrumentColor() const { return m_colors[kInstrument]; }
    void setInstrumentColor(const QColor& c) { setColorAt(kInstrument, c); }
    QColor volumeColor() const { return m_colors[kVolume]; }
    void setVolumeColor(const QColor& c) { setColorAt(kVolume, c); }
    QColor effectColor() const { return m_colors[kEffect]; }
    void setEffectColor(const QColor& c) { setColorAt(kEffect, c); }
    QColor paramColor() const { return m_colors[kParam]; }
    void setParamColor(const QColor& c) { setColorAt(kParam, c); }
    QColor emptyColor() const { return m_colors[kEmpty]; }
    void setEmptyColor(const QColor& c) { setColorAt(kEmpty, c); }
    QColor numberColor() const { return m_colors[kNumber]; }
    void setNumberColor(const QColor& c) { setColorAt(kNumber, c); }
    QColor cursorColor() const { return m_colors[kCursor]; }
    void setCursorColor(const QColor& c) { setColorAt(kCursor, c); }
    QColor cursorLineColor() const { return m_colors[kCursorLine]; }
    void setCursorLineColor(const QColor& c) { setColorAt(kCursorLine, c); }
    QColor lineColor() const { return m_colors[kLine]; }
    void setLineColor(const QColor& c) { setColorAt(kLine, c); }

signals:
    void sessionChanged();
    void fontChanged();
    void layoutChanged();
    void colorsChanged();

protected:
    QSGNode* updatePaintNode(QSGNode* old, UpdatePaintNodeData*) override;

private:
    // Indices into m_colors: the cell classes in the bridge's order, then the chrome.
    enum Role {
        kEmpty = RvCellClass_Empty,
        kNote = RvCellClass_Note,
        kNoteOff = RvCellClass_NoteOff,
        kInstrument = RvCellClass_Instrument,
        kVolume = RvCellClass_Volume,
        kEffect = RvCellClass_Effect,
        kParam = RvCellClass_Param,
        kCustom = RvCellClass_Custom,
        kNumber,
        kCursor,
        kCursorLine,
        kLine,
        kRoleCount
    };
    void setColorAt(int role, const QColor& color);
    void connectSession();

    Session* m_session = nullptr;
    std::vector<QMetaObject::Connection> m_connections;
    QFont m_font;
    qreal m_rowHeight = 26.0;
    qreal m_numberWidth = 44.0;
    qreal m_numberPadding = 14.0;
    qreal m_fade = 0.8;
    QColor m_colors[kRoleCount];
    // Render-thread scratch: the rows on screen, resized only when the window's shape or
    // the item's height changes.
    std::vector<RvGridCell> m_cells;
};
