package eventsystem

var globalEventListener = NewEventListener()

func init() {
	for range 3 {
		globalEventListener.StartNewEventLoop()
	}
}

func Global() *EventListener {
	return globalEventListener
}
