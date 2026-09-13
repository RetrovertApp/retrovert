#pragma once

#include <QColor>
#include <QQuickItem>
#include <QtQml/qqmlregistration.h>
#include <vector>

#include "session.h"

// The channel meter: one square per scope channel, lit by its decayed VU level, laid out in
// `columns` columns. The levels come from the Rust bridge already decayed; this item only
// draws a grid of quads with per-vertex alpha, one node, one upload per frame.
class Vu : public QQuickItem {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(Session* session READ session WRITE setSession NOTIFY sessionChanged)
    Q_PROPERTY(int columns READ columns WRITE setColumns NOTIFY columnsChanged)
    Q_PROPERTY(qreal gap READ gap WRITE setGap NOTIFY gapChanged)
    Q_PROPERTY(qreal floor READ floor WRITE setFloor NOTIFY floorChanged)
    Q_PROPERTY(QColor color READ color WRITE setColor NOTIFY colorChanged)
    Q_PROPERTY(int channels READ channels NOTIFY channelsChanged)

public:
    explicit Vu(QQuickItem* parent = nullptr);

    Session* session() const { return m_session; }
    void setSession(Session* session);
    // Columns of the grid; 0 picks the smallest square that holds every channel.
    int columns() const { return m_columns; }
    void setColumns(int columns);
    qreal gap() const { return m_gap; }
    void setGap(qreal gap);
    // The alpha of a silent cell, so the grid stays visible.
    qreal floor() const { return m_floor; }
    void setFloor(qreal floor);
    QColor color() const { return m_color; }
    void setColor(const QColor& color);
    // Channels drawn in the last frame.
    int channels() const { return m_channels; }

signals:
    void sessionChanged();
    void columnsChanged();
    void gapChanged();
    void floorChanged();
    void colorChanged();
    void channelsChanged();

protected:
    QSGNode* updatePaintNode(QSGNode* old, UpdatePaintNodeData*) override;

private:
    Session* m_session = nullptr;
    QMetaObject::Connection m_frame;
    int m_columns = 0;
    qreal m_gap = 2.0;
    qreal m_floor = 0.08;
    QColor m_color = Qt::white;
    int m_channels = 0;
    // Render-thread scratch, sized once; refilled in place every frame.
    std::vector<float> m_levels;
};
