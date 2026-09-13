#pragma once

#include <QColor>
#include <QQuickItem>
#include <QtQml/qqmlregistration.h>
#include <vector>

#include "retrovert_ui.h"
#include "session.h"

// One oscilloscope channel drawn as a line strip straight into the scene graph. The points
// come from the Rust bridge already normalised; this item only scales them to its box.
class Scope : public QQuickItem {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(Session* session READ session WRITE setSession NOTIFY sessionChanged)
    Q_PROPERTY(int channel READ channel WRITE setChannel NOTIFY channelChanged)
    Q_PROPERTY(QColor color READ color WRITE setColor NOTIFY colorChanged)
    Q_PROPERTY(qreal lineWidth READ lineWidth WRITE setLineWidth NOTIFY lineWidthChanged)

public:
    explicit Scope(QQuickItem* parent = nullptr);

    Session* session() const { return m_session; }
    void setSession(Session* session);
    int channel() const { return m_channel; }
    void setChannel(int channel);
    QColor color() const { return m_color; }
    void setColor(const QColor& color);
    qreal lineWidth() const { return m_lineWidth; }
    void setLineWidth(qreal width);

signals:
    void sessionChanged();
    void channelChanged();
    void colorChanged();
    void lineWidthChanged();

protected:
    QSGNode* updatePaintNode(QSGNode* old, UpdatePaintNodeData*) override;

private:
    Session* m_session = nullptr;
    QMetaObject::Connection m_frame;
    int m_channel = 0;
    QColor m_color = Qt::white;
    qreal m_lineWidth = 1.0;
    // Render-thread scratch, sized once; refilled in place every frame.
    std::vector<RvScopePoint> m_points;
};
