// Package dbutil holds small helpers shared by the repository layer that are
// too generic to belong to any one aggregate's repository.
package dbutil

import "sort"

// UniqueStrings returns the sorted, deduplicated form of values. Repeated by
// hand across repositories before being centralized here.
func UniqueStrings(values []string) []string {
	unique := make(map[string]struct{}, len(values))
	for _, value := range values {
		unique[value] = struct{}{}
	}
	result := make([]string, 0, len(unique))
	for value := range unique {
		result = append(result, value)
	}
	sort.Strings(result)
	return result
}
