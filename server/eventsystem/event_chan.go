package eventsystem

type eventPair struct {
	Event        Event
	EventHandler []EventHandler
}

type eventChan chan eventPair

// newEventChan creates a new eventChan with the specified buffer size.
// The parameter size dictates how many events can be queued before producers block.
func newEventChan(size int) eventChan {
	return make(eventChan, size)
}

// close safely closes the event channel.
// Any subsequent send operations to this channel will panic.
func (ch eventChan) close() { // channel is a pointer type, so don't use a pointer receiver here
	close(ch)
}
