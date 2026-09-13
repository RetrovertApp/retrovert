#include "pattern.h"

#include <QFontMetricsF>
#include <QQuickWindow>
#include <QSGFlatColorMaterial>
#include <QSGGeometryNode>
#include <QSGSimpleRectNode>
#include <QSGTextNode>
#include <QTextLayout>
#include <QTextOption>
#include <algorithm>
#include <cmath>

namespace {
// The glyph runs of every visible row live in one text node, rebuilt when the rows change.
struct Nodes : QSGNode {
    QSGSimpleRectNode* band = nullptr;
    QSGSimpleRectNode* bandTop = nullptr;
    QSGSimpleRectNode* bandBottom = nullptr;
    QSGGeometryNode* separators = nullptr;
    QSGTextNode* text = nullptr;
};

QString cellText(const RvGridCell& cell) {
    const auto* bytes = reinterpret_cast<const char*>(cell.text);
    const size_t length = strnlen(bytes, sizeof(cell.text));
    return QString::fromUtf8(bytes, static_cast<int>(length));
}

QColor faded(QColor color, qreal alpha) {
    color.setAlphaF(color.alphaF() * alpha);
    return color;
}
}

Pattern::Pattern(QQuickItem* parent) : QQuickItem(parent) {
    setFlag(ItemHasContents, true);
    for (QColor& color : m_colors) color = Qt::white;
    m_colors[kCursor] = QColor(0, 0, 0, 0);
}

void Pattern::setSession(Session* session) {
    if (session == m_session) return;
    for (const QMetaObject::Connection& c : m_connections) disconnect(c);
    m_connections.clear();
    m_session = session;
    connectSession();
    emit sessionChanged();
    update();
}

void Pattern::connectSession() {
    if (!m_session) return;
    // The cursor row moves with the position, the cells with the pattern, the shape with the song.
    m_connections.push_back(connect(m_session, &Session::positionChanged, this, &QQuickItem::update));
    m_connections.push_back(connect(m_session, &Session::patternChanged, this, &QQuickItem::update));
    m_connections.push_back(connect(m_session, &Session::stateChanged, this, &QQuickItem::update));
}

void Pattern::setFont(const QFont& font) {
    if (font == m_font) return;
    m_font = font;
    emit fontChanged();
    update();
}

void Pattern::setRowHeight(qreal height) {
    if (qFuzzyCompare(height, m_rowHeight)) return;
    m_rowHeight = height;
    emit layoutChanged();
    update();
}

void Pattern::setNumberWidth(qreal width) {
    if (qFuzzyCompare(width, m_numberWidth)) return;
    m_numberWidth = width;
    emit layoutChanged();
    update();
}

void Pattern::setNumberPadding(qreal padding) {
    if (qFuzzyCompare(padding, m_numberPadding)) return;
    m_numberPadding = padding;
    emit layoutChanged();
    update();
}

void Pattern::setFade(qreal fade) {
    if (qFuzzyCompare(fade, m_fade)) return;
    m_fade = fade;
    emit layoutChanged();
    update();
}

void Pattern::setColorAt(int role, const QColor& color) {
    if (m_colors[role] == color) return;
    m_colors[role] = color;
    emit colorsChanged();
    update();
}

QSGNode* Pattern::updatePaintNode(QSGNode* old, UpdatePaintNodeData*) {
    auto* nodes = static_cast<Nodes*>(old);
    if (!nodes) {
        nodes = new Nodes;
        nodes->band = new QSGSimpleRectNode;
        nodes->bandTop = new QSGSimpleRectNode;
        nodes->bandBottom = new QSGSimpleRectNode;
        nodes->separators = new QSGGeometryNode;
        auto* geometry = new QSGGeometry(QSGGeometry::defaultAttributes_Point2D(), 0);
        geometry->setDrawingMode(QSGGeometry::DrawLines);
        nodes->separators->setGeometry(geometry);
        nodes->separators->setFlag(QSGNode::OwnsGeometry);
        nodes->separators->setMaterial(new QSGFlatColorMaterial);
        nodes->separators->setFlag(QSGNode::OwnsMaterial);
        nodes->text = window()->createTextNode();
        nodes->text->setRenderType(QSGTextNode::NativeRendering);
        nodes->appendChildNode(nodes->band);
        nodes->appendChildNode(nodes->bandTop);
        nodes->appendChildNode(nodes->bandBottom);
        nodes->appendChildNode(nodes->separators);
        nodes->appendChildNode(nodes->text);
    }

    const float w = static_cast<float>(width());
    const float h = static_cast<float>(height());
    const RvState state = m_session ? m_session->state() : RvState{};
    const int channels = static_cast<int>(state.pattern_channels);
    const int columns = static_cast<int>(state.pattern_columns);
    const bool shown = m_session && channels > 0 && columns > 0 && state.has_position && h > 0 && m_rowHeight > 0;

    nodes->text->clear();
    if (!shown) {
        nodes->band->setRect(0, 0, 0, 0);
        nodes->bandTop->setRect(0, 0, 0, 0);
        nodes->bandBottom->setRect(0, 0, 0, 0);
        nodes->separators->geometry()->allocate(0);
        nodes->separators->markDirty(QSGNode::DirtyGeometry);
        return nodes;
    }

    // An odd number of rows so the cursor row sits in the middle, the block centred vertically.
    int rows = static_cast<int>(std::ceil(h / m_rowHeight));
    if (rows % 2 == 0) rows += 1;
    const int centre = rows / 2;
    const float top = (h - static_cast<float>(rows) * static_cast<float>(m_rowHeight)) / 2.0f;
    const int64_t first = static_cast<int64_t>(state.row) - centre;
    const int stride = channels * columns;
    const size_t needed = static_cast<size_t>(rows) * static_cast<size_t>(stride);
    if (m_cells.size() != needed) m_cells.assign(needed, RvGridCell{});
    else std::fill(m_cells.begin(), m_cells.end(), RvGridCell{});
    // Rows before the pattern's first are blank; the rest come from the bridge in one block.
    const int64_t lo = std::max<int64_t>(first, 0);
    const size_t skip = static_cast<size_t>(lo - first) * static_cast<size_t>(stride);
    if (skip < needed) {
        m_session->patternCells(static_cast<uint32_t>(lo), static_cast<uint32_t>(first + rows),
                                m_cells.data() + skip, static_cast<uint32_t>(needed - skip));
    }

    const float channelWidth = (w - static_cast<float>(m_numberWidth)) / static_cast<float>(channels);
    const QFontMetricsF metrics(m_font);
    const qreal charWidth = std::max<qreal>(1.0, metrics.horizontalAdvance(QLatin1Char('0')));
    const int chars = static_cast<int>(std::floor((channelWidth - 8.0f) / charWidth));

    // Which leading columns fit, from the widest text each column holds on screen.
    std::vector<int> widths(static_cast<size_t>(columns), 0);
    for (size_t i = 0; i < needed; ++i) {
        const auto* bytes = reinterpret_cast<const char*>(m_cells[i].text);
        widths[i % static_cast<size_t>(columns)] = std::max(
            widths[i % static_cast<size_t>(columns)], static_cast<int>(strnlen(bytes, sizeof(m_cells[i].text))));
    }
    int shownColumns = 0;
    int used = 0;
    for (int c = 0; c < columns; ++c) {
        const int need = widths[static_cast<size_t>(c)] + (c > 0 ? 1 : 0);
        if (c > 0 && used + need > chars) break;
        used += need;
        shownColumns += 1;
    }

    // Cursor band, then the channel separators.
    const float bandY = top + static_cast<float>(centre) * static_cast<float>(m_rowHeight);
    nodes->band->setRect(0, bandY, w, static_cast<float>(m_rowHeight));
    nodes->band->setColor(m_colors[kCursor]);
    nodes->bandTop->setRect(0, bandY, w, 1);
    nodes->bandTop->setColor(m_colors[kCursorLine]);
    nodes->bandBottom->setRect(0, bandY + static_cast<float>(m_rowHeight) - 1, w, 1);
    nodes->bandBottom->setColor(m_colors[kCursorLine]);
    {
        auto* geometry = nodes->separators->geometry();
        geometry->allocate(channels * 2);
        auto* v = geometry->vertexDataAsPoint2D();
        for (int c = 0; c < channels; ++c) {
            const float x = std::round(static_cast<float>(m_numberWidth) + static_cast<float>(c) * channelWidth) + 0.5f;
            v[c * 2].set(x, 0);
            v[c * 2 + 1].set(x, h);
        }
        nodes->separators->markDirty(QSGNode::DirtyGeometry);
        auto* material = static_cast<QSGFlatColorMaterial*>(nodes->separators->material());
        if (material->color() != m_colors[kLine]) {
            material->setColor(m_colors[kLine]);
            nodes->separators->markDirty(QSGNode::DirtyMaterial);
        }
    }

    // One text layout per row: the row number, then each channel's cells joined by spaces,
    // each channel its own line placed at its column's centre, coloured by cell class.
    const qreal lineY = (m_rowHeight - metrics.height()) / 2.0;
    QString text;
    QList<QTextLayout::FormatRange> formats;
    for (int r = 0; r < rows; ++r) {
        const int64_t row = first + r;
        const qreal distance = static_cast<qreal>(std::abs(r - centre)) / static_cast<qreal>(std::max(1, centre));
        const qreal alpha = 1.0 - m_fade * std::pow(distance, 1.5);
        text.clear();
        formats.clear();
        std::vector<std::pair<qreal, int>> lines;  // x position and character count per line
        if (row >= 0) {
            const QString number = QStringLiteral("%1").arg(row, 2, 16, QLatin1Char('0')).toUpper();
            QTextLayout::FormatRange range;
            range.start = 0;
            range.length = number.size();
            range.format.setForeground(faded(m_colors[kNumber], alpha));
            formats.push_back(range);
            text += number;
            lines.emplace_back(m_numberPadding, number.size());
        }
        const RvGridCell* cells = m_cells.data() + static_cast<size_t>(r) * static_cast<size_t>(stride);
        for (int ch = 0; ch < channels; ++ch) {
            const int start = text.size();
            for (int c = 0; c < shownColumns; ++c) {
                const RvGridCell& cell = cells[ch * columns + c];
                QString piece = cellText(cell);
                if (piece.size() < widths[static_cast<size_t>(c)]) piece = piece.leftJustified(widths[static_cast<size_t>(c)]);
                if (c > 0) text += QLatin1Char(' ');
                const uint8_t cls = cell.class_ < kNumber ? cell.class_ : RvCellClass_Custom;
                QTextLayout::FormatRange range;
                range.start = text.size();
                range.length = piece.size();
                range.format.setForeground(faded(m_colors[cls], alpha));
                formats.push_back(range);
                text += piece;
            }
            const int count = text.size() - start;
            const qreal textWidth = static_cast<qreal>(count) * charWidth;
            const qreal x = m_numberWidth + static_cast<qreal>(ch) * channelWidth + (channelWidth - textWidth) / 2.0;
            lines.emplace_back(x, count);
        }

        QTextLayout layout(text, m_font);
        layout.setCacheEnabled(false);
        // Each line takes exactly the characters it is given: no breaking at spaces.
        QTextOption option;
        option.setWrapMode(QTextOption::WrapAnywhere);
        layout.setTextOption(option);
        layout.setFormats(formats);
        layout.beginLayout();
        for (const auto& [x, count] : lines) {
            QTextLine line = layout.createLine();
            if (!line.isValid()) break;
            line.setNumColumns(count);
            line.setPosition(QPointF(x, lineY));
        }
        layout.endLayout();
        nodes->text->addTextLayout(QPointF(0, top + static_cast<qreal>(r) * m_rowHeight), &layout);
    }
    return nodes;
}
