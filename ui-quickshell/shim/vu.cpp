#include "vu.h"

#include <QSGGeometryNode>
#include <QSGVertexColorMaterial>
#include <cmath>

namespace {
constexpr uint32_t kChannelCapacity = 64;
constexpr int kVerticesPerCell = 6;
}

Vu::Vu(QQuickItem* parent) : QQuickItem(parent), m_levels(kChannelCapacity) {
    setFlag(ItemHasContents, true);
}

void Vu::setSession(Session* session) {
    if (session == m_session) return;
    if (m_frame) disconnect(m_frame);
    m_session = session;
    if (m_session) m_frame = connect(m_session, &Session::frame, this, &QQuickItem::update);
    emit sessionChanged();
    update();
}

void Vu::setColumns(int columns) {
    if (columns == m_columns) return;
    m_columns = columns;
    emit columnsChanged();
    update();
}

void Vu::setGap(qreal gap) {
    if (qFuzzyCompare(gap, m_gap)) return;
    m_gap = gap;
    emit gapChanged();
    update();
}

void Vu::setFloor(qreal floor) {
    if (qFuzzyCompare(floor, m_floor)) return;
    m_floor = floor;
    emit floorChanged();
    update();
}

void Vu::setColor(const QColor& color) {
    if (color == m_color) return;
    m_color = color;
    emit colorChanged();
    update();
}

QSGNode* Vu::updatePaintNode(QSGNode* old, UpdatePaintNodeData*) {
    auto* node = static_cast<QSGGeometryNode*>(old);
    if (!node) {
        node = new QSGGeometryNode;
        auto* geometry = new QSGGeometry(QSGGeometry::defaultAttributes_ColoredPoint2D(), 0);
        geometry->setDrawingMode(QSGGeometry::DrawTriangles);
        node->setGeometry(geometry);
        node->setFlag(QSGNode::OwnsGeometry);
        auto* material = new QSGVertexColorMaterial;
        node->setMaterial(material);
        node->setFlag(QSGNode::OwnsMaterial);
    }

    const uint32_t count = m_session ? m_session->vuLevels(m_levels.data(), kChannelCapacity) : 0;
    if (static_cast<int>(count) != m_channels) {
        m_channels = static_cast<int>(count);
        // Queued: the render thread must not touch QML bindings directly.
        QMetaObject::invokeMethod(this, &Vu::channelsChanged, Qt::QueuedConnection);
    }

    const int columns = m_columns > 0 ? m_columns
        : std::max(1, static_cast<int>(std::ceil(std::sqrt(static_cast<double>(count)))));
    const int rows = count == 0 ? 0 : (static_cast<int>(count) + columns - 1) / columns;
    const float gap = static_cast<float>(m_gap);
    const float cellW = (static_cast<float>(width()) - gap * static_cast<float>(columns - 1)) / static_cast<float>(columns);
    const float cellH = rows == 0 ? 0.0f
        : (static_cast<float>(height()) - gap * static_cast<float>(rows - 1)) / static_cast<float>(rows);

    auto* geometry = node->geometry();
    geometry->allocate(static_cast<int>(count) * kVerticesPerCell);
    auto* vertices = geometry->vertexDataAsColoredPoint2D();
    const uchar r = static_cast<uchar>(m_color.red());
    const uchar g = static_cast<uchar>(m_color.green());
    const uchar b = static_cast<uchar>(m_color.blue());
    const float floorAlpha = static_cast<float>(m_floor);
    for (uint32_t i = 0; i < count; ++i) {
        const int col = static_cast<int>(i) % columns;
        const int row = static_cast<int>(i) / columns;
        const float x0 = static_cast<float>(col) * (cellW + gap);
        const float y0 = static_cast<float>(row) * (cellH + gap);
        const float x1 = x0 + cellW;
        const float y1 = y0 + cellH;
        const float level = std::max(floorAlpha, std::min(1.0f, m_levels[i]));
        // Premultiplied, as the vertex-colour material expects.
        const uchar a = static_cast<uchar>(std::lround(level * static_cast<float>(m_color.alpha())));
        const uchar pr = static_cast<uchar>(r * a / 255);
        const uchar pg = static_cast<uchar>(g * a / 255);
        const uchar pb = static_cast<uchar>(b * a / 255);
        auto* v = vertices + i * kVerticesPerCell;
        v[0].set(x0, y0, pr, pg, pb, a);
        v[1].set(x1, y0, pr, pg, pb, a);
        v[2].set(x0, y1, pr, pg, pb, a);
        v[3].set(x1, y0, pr, pg, pb, a);
        v[4].set(x1, y1, pr, pg, pb, a);
        v[5].set(x0, y1, pr, pg, pb, a);
    }
    node->markDirty(QSGNode::DirtyGeometry);
    return node;
}
