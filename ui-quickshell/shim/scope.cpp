#include "scope.h"

#include <QSGFlatColorMaterial>
#include <QSGGeometryNode>

#include "session.h"

namespace {
constexpr uint32_t kPointCapacity = 1024;
}

Scope::Scope(QQuickItem* parent) : QQuickItem(parent), m_points(kPointCapacity) {
    setFlag(ItemHasContents, true);
}

void Scope::setSession(Session* session) {
    if (session == m_session) return;
    if (m_frame) disconnect(m_frame);
    m_session = session;
    if (m_session) m_frame = connect(m_session, &Session::frame, this, &QQuickItem::update);
    emit sessionChanged();
    update();
}

void Scope::setChannel(int channel) {
    if (channel == m_channel) return;
    m_channel = channel;
    emit channelChanged();
    update();
}

void Scope::setColor(const QColor& color) {
    if (color == m_color) return;
    m_color = color;
    emit colorChanged();
    update();
}

void Scope::setLineWidth(qreal width) {
    if (qFuzzyCompare(width, m_lineWidth)) return;
    m_lineWidth = width;
    emit lineWidthChanged();
    update();
}

QSGNode* Scope::updatePaintNode(QSGNode* old, UpdatePaintNodeData*) {
    auto* node = static_cast<QSGGeometryNode*>(old);
    if (!node) {
        node = new QSGGeometryNode;
        auto* geometry = new QSGGeometry(QSGGeometry::defaultAttributes_Point2D(), 0);
        geometry->setDrawingMode(QSGGeometry::DrawLineStrip);
        node->setGeometry(geometry);
        node->setFlag(QSGNode::OwnsGeometry);
        auto* material = new QSGFlatColorMaterial;
        node->setMaterial(material);
        node->setFlag(QSGNode::OwnsMaterial);
    }

    const uint32_t count = m_session && m_channel >= 0
        ? m_session->scopePoints(static_cast<uint32_t>(m_channel), m_points.data(), kPointCapacity)
        : 0;

    auto* geometry = node->geometry();
    geometry->allocate(static_cast<int>(count));
    geometry->setLineWidth(static_cast<float>(m_lineWidth));
    auto* vertices = geometry->vertexDataAsPoint2D();
    const float w = static_cast<float>(width());
    const float h = static_cast<float>(height());
    for (uint32_t i = 0; i < count; ++i) {
        // y runs -1..1 with positive up; the item's y axis points down.
        vertices[i].set(m_points[i].x * w, (0.5f - 0.5f * m_points[i].y) * h);
    }
    node->markDirty(QSGNode::DirtyGeometry);

    auto* material = static_cast<QSGFlatColorMaterial*>(node->material());
    if (material->color() != m_color) {
        material->setColor(m_color);
        node->markDirty(QSGNode::DirtyMaterial);
    }
    return node;
}
